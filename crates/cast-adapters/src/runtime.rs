use crate::{
    auth::{self, Identity, Invitation},
    capabilities::Profile,
    discovery, dlna,
    media_http::{self, Resource},
    protocol::{MAX_FRAME, Message},
    rtc_recovery::{CAPABILITY, RtcRecovery},
    session::Session,
};
use anyhow::{Context, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex as StdMutex, mpsc as std_mpsc},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_tungstenite::{
    Connector, WebSocketStream,
    tungstenite::{Message as Frame, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Events = std_mpsc::SyncSender<String>;
fn emit(events: &Events, value: Value) {
    let _ = events.try_send(value.to_string());
}
fn event(events: &Events, kind: &str, body: Value) {
    emit(events, json!({"type":kind,"body":body}));
}
fn wire_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
        .max_write_buffer_size(512 * 1024)
}
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
    client: Option<mpsc::Sender<Message>>,
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
struct Server {
    identity: Arc<Identity>,
    invitation: Mutex<Invitation>,
    pending: Mutex<HashMap<Uuid, oneshot::Sender<bool>>>,
    session: Mutex<Option<Session>>,
    connections: Mutex<HashMap<Uuid, mpsc::Sender<Message>>>,
    events: Events,
    variant: String,
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
                let server = Arc::new(Server {
                    identity: self.identity.clone(),
                    invitation: Mutex::new(Invitation::new(Instant::now())),
                    pending: Mutex::new(HashMap::new()),
                    session: Mutex::new(None),
                    connections: Mutex::new(HashMap::new()),
                    events: self.events.clone(),
                    variant: c["variant"].as_str().unwrap_or("standard").into(),
                });
                let advertisement = if let IpAddr::V4(ip) = address.ip() {
                    Some(discovery::Advertisement::start(
                        &self.identity.id.to_string(),
                        c["name"].as_str().unwrap_or("LanCast"),
                        ip,
                        address.port(),
                        &server.variant,
                    )?)
                } else {
                    None
                };
                event(
                    &self.events,
                    "receiver.ready",
                    json!({"address":address.to_string(),"fingerprint":self.identity.fingerprint,"invite":server.invitation.lock().await.token(),"deviceId":self.identity.id,"expiresInMs":120000,"trust":"session_only"}),
                );
                let task_server = server.clone();
                self.tasks.push(tokio::spawn(async move {
                    let _advertisement = advertisement;
                    let _ = serve(listener, task_server).await;
                }));
                self.server = Some(server);
            }
            "invite" => {
                let server = self.server.as_ref().context("NOT_LISTENING")?;
                let mut invitation = server.invitation.lock().await;
                *invitation = Invitation::new(Instant::now());
                event(
                    &self.events,
                    "receiver.invite",
                    json!({"invite":invitation.token(),"expiresInMs":120000}),
                );
            }
            "approve" => {
                let server = self.server.as_ref().context("NOT_LISTENING")?;
                let id = Uuid::parse_str(string(&c, "connectionId")?)?;
                if let Some(tx) = server.pending.lock().await.remove(&id) {
                    let _ = tx.send(c["accept"].as_bool().unwrap_or(false));
                }
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
                ensure!(self.client.is_none(), "ALREADY_CONNECTED");
                let (tx, task) = connect(
                    self.identity.clone(),
                    string(&c, "address")?,
                    string(&c, "fingerprint")?,
                    string(&c, "invite")?,
                    c["name"].as_str().unwrap_or("LanCast Sender"),
                    self.events.clone(),
                )
                .await?;
                self.client = Some(tx);
                self.tasks.push(task);
            }
            "send" => {
                let msg = Message::parse(&c["message"].to_string())?;
                if let Some(tx) = &self.client {
                    tx.try_send(msg).context("SIGNAL_QUEUE_FULL")?;
                } else if let Some(server) = &self.server {
                    let mut session = server.session.lock().await;
                    let active = session.as_mut().context("NO_SESSION")?;
                    ensure!(msg.session_id == Some(active.id), "SESSION_EXPIRED");
                    ensure!(
                        matches!(
                            msg.kind.as_str(),
                            "rtc.answer"
                                | "rtc.ice"
                                | "rtc.state"
                                | "statistics"
                                | "session.state"
                                | "session.stop"
                                | "error"
                        ),
                        "INVALID_DIRECTION"
                    );
                    if matches!(msg.kind.as_str(), "rtc.answer" | "rtc.ice" | "rtc.state")
                        && msg.body["negotiationId"]
                            .as_str()
                            .and_then(|v| Uuid::parse_str(v).ok())
                            != active.negotiation
                    {
                        return Ok(());
                    }
                    if msg.kind == "rtc.state" && !active.rtc_recovery {
                        return Ok(());
                    }
                    if msg.kind == "session.state" && msg.body["state"] == "ready" {
                        if active.rtc_recovery
                            && msg.body["negotiationId"]
                                .as_str()
                                .and_then(|s| Uuid::parse_str(s).ok())
                                != active.negotiation
                        {
                            return Ok(());
                        }
                        active.ready()?;
                    }
                    if let Some(tx) = server.connections.lock().await.get(&active.owner) {
                        tx.try_send(msg.clone()).context("SIGNAL_QUEUE_FULL")?;
                    }
                    if msg.kind == "session.stop" {
                        active.stop();
                        *session = None;
                    }
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
                self.client = None;
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
                    let mut guard = server.session.lock().await;
                    if let Some(s) = guard.as_mut() {
                        let mut msg =
                            Message::new("session.stop", json!({"reason":"receiver_stopped"}));
                        msg.session_id = Some(s.id);
                        if let Some(tx) = server.connections.lock().await.get(&s.owner) {
                            let _ = tx.try_send(msg);
                        }
                        s.stop();
                    }
                    *guard = None;
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
// The handshake callback signature is imposed by tungstenite's public API.
#[allow(clippy::result_large_err)]
async fn serve(listener: TcpListener, server: Arc<Server>) -> anyhow::Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server.identity.server_config()?));
    let slots = Arc::new(Semaphore::new(8));
    loop {
        let (stream, address) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let acceptor = acceptor.clone();
        let server = server.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let handshake = async {
                let tls = acceptor.accept(stream).await?;
                let ws=tokio_tungstenite::accept_hdr_async_with_config(tls,|request:&tokio_tungstenite::tungstenite::handshake::server::Request,response:tokio_tungstenite::tungstenite::handshake::server::Response|{
            if request.uri().path()!="/v1/ws"||request.headers().contains_key("origin"){Err(tokio_tungstenite::tungstenite::http::Response::builder().status(403).body(Some("Native control endpoint".into())).unwrap())}else{Ok(response)}
        },Some(wire_config())).await?;
                Ok::<_, anyhow::Error>(ws)
            };
            if let Ok(Ok(ws)) = tokio::time::timeout(Duration::from_secs(8), handshake).await {
                let _ = receiver_connection(ws, address.ip(), server).await;
            }
        });
    }
}
async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    message: &Message,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(5),
        ws.send(Frame::Text(serde_json::to_string(message)?.into())),
    )
    .await??;
    Ok(())
}
async fn receive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
) -> anyhow::Result<Message> {
    loop {
        match ws.next().await.context("DISCONNECTED")?? {
            Frame::Text(text) => return Message::parse(&text),
            Frame::Ping(data) => ws.send(Frame::Pong(data)).await?,
            Frame::Pong(_) => {}
            _ => anyhow::bail!("DISCONNECTED"),
        }
    }
}
async fn receiver_connection<
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
>(
    mut ws: WebSocketStream<S>,
    ip: IpAddr,
    server: Arc<Server>,
) -> anyhow::Result<()> {
    let nonce = auth::random_token();
    send(
        &mut ws,
        &Message::new("auth.challenge", json!({"nonce":nonce,"expiresInMs":30000})),
    )
    .await?;
    let pair = tokio::time::timeout(Duration::from_secs(30), receive(&mut ws)).await??;
    ensure!(pair.kind == "pair.request", "AUTH_REQUIRED");
    let owner = Uuid::parse_str(pair.string("senderDeviceId")?)?;
    let public = pair.string("senderPublicKey")?;
    auth::verify(public, &nonce, owner, pair.string("signature")?)?;
    server
        .invitation
        .lock()
        .await
        .consume(pair.string("invite")?, Instant::now())?;
    let connection = Uuid::new_v4();
    let (tx, rx) = oneshot::channel();
    server.pending.lock().await.insert(connection, tx);
    event(
        &server.events,
        "pair.request",
        json!({"connectionId":connection,"senderDeviceId":owner,"senderName":pair.string("senderName")?.chars().take(80).collect::<String>(),"address":ip.to_string()}),
    );
    let accepted = tokio::time::timeout(Duration::from_secs(60), rx).await;
    server.pending.lock().await.remove(&connection);
    if !matches!(accepted, Ok(Ok(true))) {
        send(&mut ws, &pair.error("PAIR_REJECTED")).await?;
        return Ok(());
    }
    send(&mut ws,&pair.reply("pair.accepted",json!({"receiverDeviceId":server.identity.id,"deviceToken":auth::random_token(),"receiverPublicKey":server.identity.public_key,"trust":"session_only"}))).await?;
    let fingerprint = {
        use base64::Engine;
        hex::encode(Sha256::digest(
            base64::engine::general_purpose::STANDARD.decode(public)?,
        ))
    };
    let (tx, mut outgoing) = mpsc::channel::<Message>(32);
    {
        let mut connections = server.connections.lock().await;
        ensure!(
            !connections.contains_key(&owner),
            "DEVICE_ALREADY_CONNECTED"
        );
        connections.insert(owner, tx);
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut last_seen = Instant::now();
    let outcome: anyhow::Result<()>=async {loop{tokio::select!{
        received=receive(&mut ws)=>{let mut request=received?;last_seen=Instant::now();if request.kind=="ping"{send(&mut ws,&request.reply("pong",request.body.clone())).await?;continue;}
            if request.kind=="pong"{continue;}
            let response=handle_request(&server,owner,ip,&fingerprint,&mut request).await.unwrap_or_else(|e|request.error(&e.to_string()));send(&mut ws,&response).await?;
        }
        message=outgoing.recv()=>{if let Some(message)=message{send(&mut ws,&message).await?;}else{break;}}
        _=heartbeat.tick()=>{ensure!(last_seen.elapsed()<Duration::from_secs(15),"HEARTBEAT_TIMEOUT");send(&mut ws,&Message::new("ping",json!({"nonce":Uuid::new_v4()}))).await?;}
    }}Ok(())}.await;
    server.connections.lock().await.remove(&owner);
    {
        let mut session = server.session.lock().await;
        if let Some(s) = session.as_mut()
            && s.owner == owner
        {
            s.stop();
            *session = None;
            event(
                &server.events,
                "session.closed",
                json!({"reason":"control_disconnected","requiresNewConsent":true}),
            );
        }
    }
    outcome
}
async fn handle_request(
    server: &Server,
    owner: Uuid,
    ip: IpAddr,
    fingerprint: &str,
    request: &mut Message,
) -> anyhow::Result<Message> {
    let mut guard = server.session.lock().await;
    if let Some(s) = guard.as_ref()
        && s.owner == owner
        && let Some(response) = s.cached(request.id, Instant::now())
    {
        return Ok(response);
    }
    if request.kind == "session.start" {
        ensure!(guard.is_none(), "BUSY");
        let mode = request.string("mode")?;
        let mut session = Session::new(owner, mode)?;
        session.rtc_recovery = mode == "mirror" && request.body["rtcRecovery"] == CAPABILITY;
        let mut response=request.reply("session.accepted",json!({"sessionId":session.id,"selectedProfile":Profile::conservative(server.variant=="legacy"),"audioPolicy":"explicit_capture_only"}));
        if session.rtc_recovery {
            response.body["rtcRecovery"] = json!(CAPABILITY);
        }
        response.session_id = Some(session.id);
        *guard = Some(session);
        guard
            .as_mut()
            .unwrap()
            .remember(request.id, response.clone(), Instant::now());
        event(
            &server.events,
            "session.started",
            json!({"sessionId":response.session_id,"mode":mode,"senderIp":ip.to_string(),"senderFingerprint":fingerprint,"rtcRecovery":response.body["rtcRecovery"]}),
        );
        return Ok(response);
    }
    if request.kind == "session.stop" && guard.is_none() {
        return Ok(request.reply("ack", json!({})));
    }
    let session = guard.as_mut().context("SESSION_EXPIRED")?;
    session.authorize(owner, request.session_id)?;
    match request.kind.as_str() {
        "rtc.offer" => {
            let previous = request
                .body
                .get("previousNegotiationId")
                .map(|v| {
                    v.as_str()
                        .context("INVALID_NEGOTIATION")
                        .and_then(|s| Ok(Uuid::parse_str(s)?))
                })
                .transpose()?;
            session.negotiate(Uuid::parse_str(request.string("negotiationId")?)?, previous)?;
        }
        "rtc.ice" => {
            ensure!(
                request.body["negotiationId"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    == session.negotiation,
                "STALE_NEGOTIATION"
            );
        }
        "file.load" => {
            ensure!(session.mode == "file", "INVALID_STATE");
            let url = url::Url::parse(request.string("url")?)?;
            ensure!(
                url.scheme() == "https"
                    && url
                        .host_str()
                        .and_then(|s| s.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                        == Some(ip)
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path().starts_with("/media/")
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "UNSAFE_MEDIA_URL"
            );
            ensure!(request.string("mime")? == "video/mp4", "MEDIA_UNSUPPORTED");
            request.body["fingerprint"] = json!(fingerprint);
        }
        "playback.command" => {
            ensure!(session.mode == "file", "INVALID_STATE");
            match request.string("action")? {
                "play" | "pause" => {}
                "seek" => ensure!(
                    request.body["positionMs"].as_u64().is_some(),
                    "INVALID_POSITION"
                ),
                "volume" => ensure!(
                    request.body["value"]
                        .as_f64()
                        .is_some_and(|n| (0.0..=1.0).contains(&n)),
                    "INVALID_VOLUME"
                ),
                _ => anyhow::bail!("UNKNOWN_COMMAND"),
            }
        }
        "session.stop" => {}
        _ => anyhow::bail!("UNKNOWN_MESSAGE"),
    }
    event(&server.events, "message", serde_json::to_value(&request)?);
    let response = request.reply("ack", json!({}));
    session.remember(request.id, response.clone(), Instant::now());
    if request.kind == "session.stop" {
        session.stop();
        *guard = None;
    }
    Ok(response)
}
async fn connect(
    identity: Arc<Identity>,
    address: &str,
    pin: &str,
    invite: &str,
    name: &str,
    events: Events,
) -> anyhow::Result<(mpsc::Sender<Message>, JoinHandle<()>)> {
    let endpoint: SocketAddr = address.parse()?;
    ensure!(!endpoint.ip().is_unspecified(), "INVALID_ADDRESS");
    let config = auth::pinned_config(pin)?;
    let url = format!("wss://{endpoint}/v1/ws");
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(8),
        tokio_tungstenite::connect_async_tls_with_config(
            url,
            Some(wire_config()),
            false,
            Some(Connector::Rustls(Arc::new(config))),
        ),
    )
    .await??;
    let challenge = tokio::time::timeout(Duration::from_secs(30), receive(&mut ws)).await??;
    ensure!(challenge.kind == "auth.challenge", "AUTH_REQUIRED");
    let pair = Message::new(
        "pair.request",
        json!({"invite":invite,"senderDeviceId":identity.id,"senderName":name,"senderPublicKey":identity.public_key,"signature":identity.sign(challenge.string("nonce")?)?}),
    );
    send(&mut ws, &pair).await?;
    event(&events, "pair.waiting", json!({}));
    let accepted = tokio::time::timeout(Duration::from_secs(65), receive(&mut ws)).await??;
    ensure!(accepted.kind == "pair.accepted", "PAIR_REJECTED");
    event(
        &events,
        "connected",
        json!({"address":address,"trust":"session_only"}),
    );
    let (tx, mut rx) = mpsc::channel::<Message>(32);
    let task = tokio::spawn(async move {
        let epoch = Instant::now();
        let mut last_seen = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut recovery: Option<RtcRecovery> = None;
        let mut pending_recovery = None;
        let result: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    request = rx.recv() => {
                        let Some(request) = request else { break };
                        if rx.is_closed() { break; }
                        let now = epoch.elapsed().as_millis() as u64;
                        if request.kind == "rtc.state" {
                            if let Some(r) = recovery.as_mut() { r.report(&request, false, now); }
                            else if request.body["state"] != "connected" {
                                let stop = request.reply("session.stop", json!({"reason":"RTC_CONNECTION_LOST"}));
                                send(&mut ws, &stop).await?;
                                event(&events, "message", serde_json::to_value(stop)?);
                            }
                            continue;
                        }
                        if request.kind == "session.start" {
                            pending_recovery = (request.body["mode"] == "mirror" && request.body["rtcRecovery"] == CAPABILITY).then_some(request.id);
                            recovery = None;
                        }
                        if request.kind == "rtc.offer" && let Some(r) = recovery.as_mut() { r.offer(&request); }
                        if request.kind == "rtc.ice" && recovery.as_ref().is_some_and(|r| !r.accepts(&request)) { continue; }
                        if request.kind == "session.stop" { recovery = None; pending_recovery = None; }
                        send(&mut ws, &request).await?;
                    }
                    response = receive(&mut ws) => {
                        let response = response?;
                        last_seen = Instant::now();
                        let now = epoch.elapsed().as_millis() as u64;
                        if response.kind == "ping" {
                            send(&mut ws, &response.reply("pong", response.body.clone())).await?;
                            continue;
                        }
                        if response.kind == "session.accepted" && pending_recovery.is_some() && response.reply_to == pending_recovery {
                            if response.body["rtcRecovery"] == CAPABILITY {
                                recovery = Some(RtcRecovery::new(response.session_id.context("SESSION_REQUIRED")?, now));
                            }
                            pending_recovery = None;
                        }
                        if response.kind == "rtc.state" {
                            if let Some(r) = recovery.as_mut() { r.report(&response, true, now); }
                            continue;
                        }
                        if matches!(response.kind.as_str(), "rtc.answer" | "rtc.ice") && recovery.as_ref().is_some_and(|r| !r.accepts(&response)) { continue; }
                        if response.kind == "session.stop" || response.kind == "error" { recovery = None; }
                        event(&events, "message", serde_json::to_value(response)?);
                    }
                    _ = tick.tick() => {
                        ensure!(last_seen.elapsed() < Duration::from_secs(15), "HEARTBEAT_TIMEOUT");
                        if let Some(message) = recovery.as_mut().and_then(|r| r.poll(epoch.elapsed().as_millis() as u64)) {
                            if message.kind == "session.stop" { send(&mut ws, &message).await?; recovery = None; }
                            event(&events, "message", serde_json::to_value(message)?);
                        }
                    }
                }
            }
            Ok(())
        }.await;
        event(
            &events,
            "disconnected",
            json!({"reason":if result.is_err(){"connection_lost"}else{"closed"},"requiresNewConsent":true}),
        );
    });
    Ok((tx, task))
}
