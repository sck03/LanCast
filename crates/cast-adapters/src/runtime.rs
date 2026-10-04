use crate::{
    auth::Identity,
    control::{Client, Server},
    discovery, dlna,
    events::{Events, event},
    media_http::{self, Resource},
    protocol::Message,
};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex as StdMutex, mpsc as std_mpsc},
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn string<'a>(v: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    v[key].as_str().with_context(|| format!("MISSING_{key}"))
}
struct QueuedCommand {
    value: Value,
    file: Option<std::fs::File>,
    generation: u64,
}
impl QueuedCommand {
    fn new(value: Value) -> anyhow::Result<Self> {
        #[cfg(unix)]
        let file = if value["op"] == "file.share" && value["path"].is_null() {
            use std::os::fd::FromRawFd;
            let fd = value["fd"].as_i64().context("FILE_REQUIRED")?;
            ensure!(fd >= 0 && fd <= i32::MAX as i64, "INVALID_FD");
            // Native caller transfers a valid detached descriptor. Queue rejection,
            // cancellation and shutdown now close it through ordinary RAII.
            Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
        } else {
            None
        };
        #[cfg(not(unix))]
        let file = None;
        Ok(Self {
            value,
            file,
            generation: 0,
        })
    }
    fn stop() -> Self {
        Self {
            value: json!({"op":"stop"}),
            file: None,
            generation: 0,
        }
    }
}
pub struct Engine {
    command: mpsc::Sender<QueuedCommand>,
    events: StdMutex<std_mpsc::Receiver<String>>,
    thread: StdMutex<Option<std::thread::JoinHandle<()>>>,
    shutdown: CancellationToken,
    stop: Arc<tokio::sync::Notify>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(feature = "sender")]
    live: Arc<StdMutex<Option<Arc<crate::live::LiveResource>>>>,
}
impl Engine {
    pub fn new() -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (command, mut receiver) = mpsc::channel::<QueuedCommand>(32);
        let (events, output) = std_mpsc::sync_channel(256);
        let shutdown = CancellationToken::new();
        let shutdown_worker = shutdown.clone();
        let stop = Arc::new(tokio::sync::Notify::new());
        let stop_worker = stop.clone();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_generation = generation.clone();
        #[cfg(feature = "sender")]
        let live = Arc::new(StdMutex::new(None));
        #[cfg(feature = "sender")]
        let live_worker = live.clone();
        let thread = std::thread::Builder::new()
            .name("lancast-control".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("runtime");
                runtime.block_on(async move {
                    let identity = match Identity::create() {
                        Ok(v) => Arc::new(v),
                        Err(_) => {
                            event(&events, "error", json!({"code":"IDENTITY_FAILED"}));
                            return;
                        }
                    };
                    let mut state = Runtime {
                        identity,
                        server: None,
                        client: None,
                        tasks: Vec::new(),
                        resources: Vec::new(),
                        renderers: HashMap::new(),
                        dlna_playing: None,
                        events: events.clone(),
                        #[cfg(feature = "legacy")]
                        bridges: Vec::new(),
                        #[cfg(feature = "sender")]
                        live: live_worker,
                        #[cfg(feature = "sender")]
                        profiles: None,
                        #[cfg(feature = "sender")]
                        live_context: None,
                        #[cfg(feature = "sender")]
                        live_task: None,
                    };
                    loop {
                        let command = tokio::select! { biased;
                            _ = shutdown_worker.cancelled() => break,
                            _ = stop_worker.notified() => Some(QueuedCommand::stop()),
                            c = receiver.recv() => c,
                        };
                        let Some(command) = command else {
                            break;
                        };
                        if command.value["op"] != "stop"
                            && command.generation
                                != worker_generation.load(std::sync::atomic::Ordering::Acquire)
                        {
                            continue;
                        }
                        // Once teardown begins, finish its bounded renderer cleanup. A duplicate
                        // stop must not detach the old cleanup and let it race a new session.
                        let result = if command.value["op"] == "stop" {
                            Some(state.command(command.value, command.file).await)
                        } else {
                            tokio::select! { biased;
                                _ = shutdown_worker.cancelled() => break,
                                _ = stop_worker.notified() => None,
                                r = state.command(command.value, command.file) => Some(r),
                            }
                        };
                        match result {
                            Some(Err(error)) => {
                                event(&events, "error", json!({"code":error.to_string()}))
                            }
                            None => {
                                let _ = state.command(json!({"op":"stop"}), None).await;
                            }
                            _ => {}
                        }
                        state.tasks.retain(|task| !task.is_finished());
                    }
                    let _ = state.command(json!({"op":"stop"}), None).await;
                    for resource in state.resources {
                        resource.revoke();
                    }
                    for task in state.tasks {
                        task.abort();
                    }
                    event(&events, "shutdown.completed", json!({}));
                });
                runtime.shutdown_timeout(Duration::from_secs(2));
            })?;
        Ok(Self {
            command,
            events: StdMutex::new(output),
            thread: StdMutex::new(Some(thread)),
            shutdown,
            stop,
            generation,
            #[cfg(feature = "sender")]
            live,
        })
    }
    pub fn command(&self, value: Value) -> anyhow::Result<()> {
        let mut command = QueuedCommand::new(value)?;
        ensure!(!self.shutdown.is_cancelled(), "ENGINE_CLOSED");
        if command.value["op"] == "stop" {
            self.generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.stop.notify_one();
            return Ok(());
        }
        if command.value["op"] == "shutdown" {
            self.shutdown();
            return Ok(());
        }
        command.generation = self.generation.load(std::sync::atomic::Ordering::Acquire);
        self.command
            .try_send(command)
            .map_err(|_| anyhow::anyhow!("COMMAND_QUEUE_FULL_OR_CLOSED"))
    }
    pub fn poll(&self) -> Option<String> {
        self.events.lock().ok()?.try_recv().ok()
    }
    pub fn close(&self) {
        self.shutdown();
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }
    #[cfg(feature = "sender")]
    pub fn write_ts(&self, data: &[u8]) -> anyhow::Result<()> {
        let live = self
            .live
            .lock()
            .map_err(|_| anyhow::anyhow!("ENGINE_CLOSED"))?
            .clone()
            .context("NO_LIVE_RESOURCE")?;
        live.write(data)
    }
}
struct Runtime {
    identity: Arc<Identity>,
    server: Option<Arc<Server>>,
    client: Option<Client>,
    tasks: Vec<JoinHandle<()>>,
    resources: Vec<Arc<Resource>>,
    renderers: HashMap<String, dlna::Renderer>,
    dlna_playing: Option<(dlna::Controller, String)>,
    events: Events,
    #[cfg(feature = "legacy")]
    bridges: Vec<Arc<crate::legacy_bridge::Bridge>>,
    #[cfg(feature = "sender")]
    live: Arc<StdMutex<Option<Arc<crate::live::LiveResource>>>>,
    #[cfg(feature = "sender")]
    profiles: Option<crate::profiles::ProfileStore>,
    #[cfg(feature = "sender")]
    live_context: Option<LiveContext>,
    #[cfg(feature = "sender")]
    live_task: Option<JoinHandle<()>>,
}
#[cfg(feature = "sender")]
struct LiveContext {
    device: dlna::Renderer,
    profile: String,
    probe: bool,
    url: String,
    generation: u64,
}
impl Runtime {
    async fn command(
        &mut self,
        c: Value,
        granted_file: Option<std::fs::File>,
    ) -> anyhow::Result<()> {
        match string(&c, "op")? {
            "listen" => {
                ensure!(self.server.is_none(), "ALREADY_LISTENING");
                let address: SocketAddr = string(&c, "address")?.parse()?;
                ensure!(!address.ip().is_unspecified(), "SELECT_LAN_INTERFACE");
                let listener = TcpListener::bind(address).await.context("PORT_IN_USE")?;
                let address = listener.local_addr()?;
                let server = Arc::new(Server::new(
                    self.identity.clone(),
                    c["variant"].as_str().unwrap_or("standard"),
                    self.events.clone(),
                ));
                let advertisement = if let IpAddr::V4(ip) = address.ip() {
                    Some(discovery::Advertisement::start(
                        &self.identity.id.to_string(),
                        c["name"].as_str().unwrap_or("LanCast"),
                        ip,
                        address.port(),
                        server.variant(),
                    )?)
                } else {
                    None
                };
                event(
                    &self.events,
                    "receiver.ready",
                    json!({"address":address.to_string(),"fingerprint":self.identity.fingerprint,"invite":server.invitation().await,"deviceId":self.identity.id,"expiresInMs":120000,"trust":"session_only"}),
                );
                let task_server = server.clone();
                self.tasks.push(tokio::spawn(async move {
                    let _advertisement = advertisement;
                    let _ = task_server.serve(listener).await;
                }));
                self.server = Some(server);
            }
            "invite" => {
                let server = self.server.as_ref().context("NOT_LISTENING")?;
                let invitation = server.renew_invitation().await;
                event(
                    &self.events,
                    "receiver.invite",
                    json!({"invite":invitation,"expiresInMs":120000}),
                );
            }
            "approve" => {
                let server = self.server.as_ref().context("NOT_LISTENING")?;
                let id = Uuid::parse_str(string(&c, "connectionId")?)?;
                server
                    .approve(id, c["accept"].as_bool().unwrap_or(false))
                    .await;
            }
            "scan" => {
                let events = self.events.clone();
                self.tasks.push(tokio::spawn(async move {
                    match tokio::task::spawn_blocking(discovery::scan_lancast).await {
                        Ok(Ok(devices)) => event(&events, "devices", json!({"devices":devices})),
                        _ => event(&events, "error", json!({"code":"DISCOVERY_FAILED"})),
                    }
                }));
            }
            "connect" => {
                ensure!(
                    self.client.as_ref().is_none_or(Client::is_closed),
                    "ALREADY_CONNECTED"
                );
                if let Some(client) = self.client.take() {
                    client.close().await;
                }
                let client = Client::connect(
                    self.identity.clone(),
                    string(&c, "address")?,
                    string(&c, "fingerprint")?,
                    string(&c, "invite")?,
                    c["name"].as_str().unwrap_or("LanCast Sender"),
                    self.events.clone(),
                )
                .await?;
                self.client = Some(client);
            }
            "send" => {
                let msg = Message::parse(&c["message"].to_string())?;
                if let Some(client) = &self.client {
                    client.send(msg)?;
                } else if let Some(server) = &self.server {
                    server.send(msg).await?;
                } else {
                    anyhow::bail!("NOT_CONNECTED");
                }
            }
            "stop" => {
                #[cfg(feature = "legacy")]
                for bridge in self.bridges.drain(..) {
                    bridge.revoke();
                }
                #[cfg(feature = "sender")]
                if let Some(live) = self.live.lock().unwrap().take() {
                    live.revoke();
                }
                #[cfg(feature = "sender")]
                {
                    self.live_context = None;
                    if let Some(task) = self.live_task.take() {
                        let _ = task.await;
                    }
                }
                if let Some(client) = self.client.take() {
                    client.close().await;
                }
                for resource in self.resources.drain(..) {
                    resource.revoke();
                }
                if let Some((controller, url)) = self.dlna_playing.take() {
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        if controller.current_uri().await? == url {
                            controller.command("stop", None).await?;
                        }
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                }
                if let Some(server) = &self.server {
                    server.stop().await;
                }
                event(&self.events, "stopped", json!({}));
            }
            "dlna.scan" => {
                ensure!(cfg!(feature = "sender"), "BACKEND_NOT_BUILT");
                let interface = string(&c, "interface")?.parse()?;
                let devices = discovery::scan_dlna(interface).await?;
                for d in &devices {
                    self.renderers.insert(d.id.clone(), d.clone());
                }
                event(&self.events, "dlna.devices", json!({"devices":devices}));
            }
            "file.share" => {
                ensure!(cfg!(feature = "sender"), "BACKEND_NOT_BUILT");
                ensure!(self.resources.is_empty(), "STOP_CURRENT_FILE_FIRST");
                let file = if let Some(file) = granted_file {
                    file
                } else if let Some(path) = c["path"].as_str() {
                    std::fs::File::open(path)?
                } else {
                    anyhow::bail!("FILE_REQUIRED")
                };
                let resource = Resource::from_file(file, string(&c, "allowedIp")?.parse()?)?;
                let encrypted = c["encrypted"].as_bool().unwrap_or(true);
                let tls = if encrypted {
                    Some(Arc::new(self.identity.server_config()?))
                } else {
                    None
                };
                let (url, task) = media_http::bind_resource(
                    resource.clone(),
                    string(&c, "address")?.parse()?,
                    tls,
                )
                .await?;
                event(
                    &self.events,
                    "file.shared",
                    json!({"url":url,"length":resource.length,"mime":"video/mp4","fingerprint":self.identity.fingerprint,"encrypted":encrypted}),
                );
                self.resources.push(resource);
                self.tasks.push(task);
            }
            "dlna.load" | "dlna.command" => {
                ensure!(cfg!(feature = "sender"), "BACKEND_NOT_BUILT");
                let device = self
                    .renderers
                    .get(string(&c, "deviceId")?)
                    .context("DLNA_UNAVAILABLE")?
                    .clone();
                let controller = dlna::Controller::new(device)?;
                #[cfg(feature = "sender")]
                if c["op"] == "dlna.load" && c["live"].as_bool().unwrap_or(false) {
                    let context = self.live_context.as_ref().context("LIVE_NOT_PREPARED")?;
                    let device = &context.device;
                    ensure!(device.id == controller.device.id, "LIVE_DEVICE_MISMATCH");
                    let live = self
                        .live
                        .lock()
                        .unwrap()
                        .as_ref()
                        .cloned()
                        .context("NO_LIVE_RESOURCE")?;
                    let url = string(&c, "url")?.to_owned();
                    ensure!(url == context.url, "LIVE_RESOURCE_MISMATCH");
                    ensure!(self.live_task.is_none(), "LIVE_ALREADY_STARTED");
                    let events = self.events.clone();
                    let probe = context.probe;
                    let generation = context.generation;
                    self.live_task = Some(tokio::spawn(async move {
                        let result = crate::live_session::run(
                            controller,
                            live,
                            url,
                            probe,
                            |kind, mut body| {
                                body["generation"] = json!(generation);
                                event(&events, kind, body)
                            },
                        )
                        .await;
                        if let Err(error) = result {
                            event(
                                &events,
                                "live.failed",
                                json!({"code":error.to_string(),"generation":generation}),
                            );
                        }
                    }));
                    return Ok(());
                }
                if c["op"] == "dlna.load" {
                    controller
                        .load_media(
                            string(&c, "url")?,
                            c["title"].as_str().unwrap_or("LanCast Video"),
                            if c["live"].as_bool().unwrap_or(false) {
                                dlna::MediaKind::LiveTs
                            } else {
                                dlna::MediaKind::FileMp4
                            },
                        )
                        .await?;
                    self.dlna_playing = Some((controller.clone(), string(&c, "url")?.to_owned()));
                    controller.command("play", None).await?;
                    event(
                        &self.events,
                        "dlna.state",
                        json!({"state":"command_accepted","firstFrameMeasured":false}),
                    );
                } else {
                    let state = controller
                        .command(string(&c, "action")?, c["positionMs"].as_u64())
                        .await?;
                    event(
                        &self.events,
                        "dlna.state",
                        json!({"response":state,"firstFrameMeasured":false}),
                    );
                }
            }
            "route.plan" => {
                let device = serde_json::from_value(c["device"].clone())?;
                let backend = serde_json::from_value(c["backend"].clone())?;
                let intent = serde_json::from_value(c["intent"].clone())?;
                event(
                    &self.events,
                    "route.plan",
                    serde_json::to_value(cast_core::plan(intent, &device, backend))?,
                );
            }
            #[cfg(feature = "legacy")]
            "bridge.create" => {
                ensure!(self.bridges.is_empty(), "BRIDGE_ALREADY_ACTIVE");
                let (url, bridge, task) = crate::legacy_bridge::Bridge::bind(
                    string(&c, "url")?,
                    string(&c, "fingerprint")?,
                )
                .await?;
                self.bridges.push(bridge);
                self.tasks.push(task);
                event(&self.events, "bridge.ready", json!({"url":url}));
            }
            #[cfg(feature = "sender")]
            "profiles.open" => {
                ensure!(self.profiles.is_none(), "PROFILES_ALREADY_OPEN");
                self.profiles = Some(crate::profiles::ProfileStore::open(
                    string(&c, "path")?.into(),
                )?);
            }
            #[cfg(feature = "sender")]
            "profile.check" => {
                let device = self
                    .renderers
                    .get(string(&c, "deviceId")?)
                    .context("DLNA_UNAVAILABLE")?;
                let profile = live_profile(c["audio"].as_bool().unwrap_or(true));
                let evidence = self.profiles.as_ref().and_then(|store| {
                    store.get(
                        &device.id,
                        &device.signature,
                        &profile,
                        crate::profiles::now(),
                    )
                });
                event(
                    &self.events,
                    "profile.checked",
                    json!({"deviceId":device.id,"profile":profile,"passed":evidence.is_some_and(|e| e.passed),"evidence":evidence,"generation":c["generation"],"requestId":c["requestId"]}),
                );
            }
            #[cfg(feature = "sender")]
            "probe.confirm" => {
                let context = self.live_context.as_ref().context("NO_PROBE")?;
                let device = &context.device;
                let profile = &context.profile;
                ensure!(context.probe, "NO_PROBE");
                let live = self
                    .live
                    .lock()
                    .unwrap()
                    .as_ref()
                    .cloned()
                    .context("NO_PROBE")?;
                ensure!(!live.cancellation().is_cancelled(), "PROBE_EXPIRED");
                let passed = c["passed"].as_bool().context("PROBE_RESULT_REQUIRED")?;
                ensure!(!passed || live.delivered() > 0, "PROBE_NOT_PULLING");
                self.profiles
                    .as_mut()
                    .context("PROFILE_STORE_REQUIRED")?
                    .save(
                        device.id.clone(),
                        crate::profiles::Evidence {
                            signature: device.signature.clone(),
                            profile: profile.clone(),
                            confirmed_at: crate::profiles::now(),
                            passed,
                            source: "user_confirmed_synthetic".into(),
                        },
                    )?;
                event(
                    &self.events,
                    "probe.saved",
                    json!({"deviceId":device.id,"passed":passed,"profile":profile}),
                );
                live.revoke();
            }
            #[cfg(feature = "sender")]
            "live.create" => {
                ensure!(self.live.lock().unwrap().is_none(), "LIVE_ALREADY_ACTIVE");
                let generation = c["generation"].as_u64().context("GENERATION_REQUIRED")?;
                let device = self
                    .renderers
                    .get(string(&c, "deviceId")?)
                    .context("DLNA_UNAVAILABLE")?
                    .clone();
                let probe = c["synthetic"].as_bool().unwrap_or(false);
                let profile = live_profile(c["audio"].as_bool().unwrap_or(true));
                ensure!(
                    probe
                        || self
                            .profiles
                            .as_ref()
                            .and_then(|store| store.get(
                                &device.id,
                                &device.signature,
                                &profile,
                                crate::profiles::now()
                            ))
                            .is_some_and(|e| e.passed),
                    "SYNTHETIC_PROBE_REQUIRED"
                );
                ensure!(
                    string(&c, "allowedIp")?.parse::<IpAddr>()? == device.ip,
                    "LIVE_DEVICE_MISMATCH"
                );
                let live = crate::live::LiveResource::new(string(&c, "allowedIp")?.parse()?);
                let (url, task) = live.clone().bind(string(&c, "address")?.parse()?).await?;
                *self.live.lock().unwrap() = Some(live);
                self.live_context = Some(LiveContext {
                    device,
                    profile,
                    probe,
                    url: url.clone(),
                    generation,
                });
                self.tasks.push(task);
                event(
                    &self.events,
                    "live.created",
                    json!({"url":url,"mime":"video/mpeg","encrypted":false,"synthetic":probe,"generation":c["generation"]}),
                );
            }
            #[cfg(feature = "sender")]
            "live.stats" => {
                let pulls = self
                    .live
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|r| r.pulls())
                    .unwrap_or(0);
                event(
                    &self.events,
                    "live.stats",
                    json!({"httpPulls":pulls,"firstFrameMeasured":false}),
                );
            }
            _ => anyhow::bail!("UNKNOWN_COMMAND"),
        }
        Ok(())
    }
}
#[cfg(feature = "sender")]
fn live_profile(audio: bool) -> String {
    format!(
        "ts-h264-baseline-1280x720-30-v1-{}",
        if audio { "aac48-stereo" } else { "silent" }
    )
}
