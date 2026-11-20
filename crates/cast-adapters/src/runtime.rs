#[cfg(feature = "dlna")]
use crate::dlna;
use crate::{
    auth::Identity,
    control::{Client, Server},
    discovery,
    events::{self, EventInbox, Events, event},
    protocol::Message,
};
#[cfg(feature = "file-sender")]
use crate::{
    blocking_io::BlockingIo,
    media_http::{self, Resource},
};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(feature = "file-proxy")]
mod bridge_resource;
mod command_context;
#[cfg(feature = "dlna-live")]
mod live_commands;

fn string<'a>(v: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    v[key].as_str().with_context(|| format!("MISSING_{key}"))
}
struct QueuedCommand {
    value: Value,
    file: Option<std::fs::File>,
    generation: u64,
}
impl QueuedCommand {
    fn new(value: Value) -> Self {
        Self {
            value,
            file: None,
            generation: 0,
        }
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
    events: EventInbox,
    thread: StdMutex<Option<std::thread::JoinHandle<()>>>,
    shutdown: CancellationToken,
    stop: Arc<tokio::sync::Notify>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(feature = "dlna-live")]
    live: Arc<StdMutex<Option<Arc<crate::live::LiveResource>>>>,
}
impl Engine {
    pub fn new() -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (command, mut receiver) = mpsc::channel::<QueuedCommand>(32);
        let shutdown = CancellationToken::new();
        let (events, output) = events::channel(256, shutdown.clone());
        let shutdown_worker = shutdown.clone();
        let stop = Arc::new(tokio::sync::Notify::new());
        let stop_worker = stop.clone();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_generation = generation.clone();
        #[cfg(feature = "dlna-live")]
        let live = Arc::new(StdMutex::new(None));
        #[cfg(feature = "dlna-live")]
        let live_worker = live.clone();
        let thread = std::thread::Builder::new()
            .name("lancast-control".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        event(
                            &events,
                            "error",
                            json!({"code":"RUNTIME_FAILED","terminal":true}),
                        );
                        return;
                    }
                };
                runtime.block_on(async move {
                    let identity = match Identity::create() {
                        Ok(v) => Arc::new(v),
                        Err(_) => {
                            event(
                                &events,
                                "error",
                                json!({"code":"IDENTITY_FAILED","terminal":true}),
                            );
                            return;
                        }
                    };
                    let mut state = Runtime {
                        identity,
                        server: None,
                        client: None,
                        tasks: Vec::new(),
                        #[cfg(feature = "file-sender")]
                        resource_tasks: Vec::new(),
                        generation: worker_generation.clone(),
                        #[cfg(feature = "file-sender")]
                        io: BlockingIo::new(worker_generation.clone(), shutdown_worker.clone()),
                        scan: discovery::ScanTask::default(),
                        #[cfg(feature = "file-sender")]
                        resources: Vec::new(),
                        #[cfg(feature = "dlna")]
                        dlna_discovery: discovery::DlnaDiscovery::default(),
                        #[cfg(feature = "dlna")]
                        dlna_playing: None,
                        events: events.clone(),
                        #[cfg(feature = "file-proxy")]
                        bridge: bridge_resource::BridgeResource::default(),
                        #[cfg(feature = "dlna-live")]
                        live: live_worker,
                        #[cfg(feature = "dlna-live")]
                        profiles: None,
                        #[cfg(feature = "dlna-live")]
                        live_context: None,
                        #[cfg(feature = "dlna-live")]
                        live_task: None,
                    };
                    loop {
                        let command = tokio::select! { biased;
                            _ = shutdown_worker.cancelled() => break,
                            _ = stop_worker.notified() => Some(QueuedCommand::stop()),
                            _ = state.complete_discovery(&events) => continue,
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
                        let context = command_context::CommandContext::new(&command.value);
                        let result = if command.value["op"] == "stop" {
                            Some(state
                                .command(command.value, command.file, command.generation)
                                .await)
                        } else {
                            tokio::select! { biased;
                                _ = shutdown_worker.cancelled() => break,
                                _ = stop_worker.notified() => None,
                                r = state.command(command.value, command.file, command.generation) => Some(r),
                            }
                        };
                        match result {
                            Some(Err(error)) => {
                                event(&events, "error", context.failure(&error))
                            }
                            None => {
                                let _ = state
                                    .command(
                                        json!({"op":"stop"}),
                                        None,
                                        worker_generation.load(std::sync::atomic::Ordering::Acquire),
                                    )
                                    .await;
                            }
                            _ => {}
                        }
                        state.tasks.retain(|task| !task.is_finished());
                        #[cfg(feature = "file-sender")]
                        state.resource_tasks.retain(|task| !task.is_finished());
                    }
                    let _ = state
                        .command(
                            json!({"op":"stop"}),
                            None,
                            worker_generation.load(std::sync::atomic::Ordering::Acquire),
                        )
                        .await;
                    #[cfg(feature = "file-sender")]
                    for resource in state.resources {
                        resource.revoke();
                    }
                    for task in state.tasks {
                        task.abort();
                    }
                    #[cfg(feature = "file-sender")]
                    for task in state.resource_tasks {
                        task.abort();
                    }
                    event(&events, "shutdown.completed", json!({}));
                });
                runtime.shutdown_timeout(Duration::from_secs(2));
            })?;
        Ok(Self {
            command,
            events: output,
            thread: StdMutex::new(Some(thread)),
            shutdown,
            stop,
            generation,
            #[cfg(feature = "dlna-live")]
            live,
        })
    }
    pub fn command(&self, value: Value) -> anyhow::Result<()> {
        self.enqueue(QueuedCommand::new(value))
    }
    pub fn share_file(&self, value: Value, file: std::fs::File) -> anyhow::Result<()> {
        ensure!(value["op"] == "file.share", "INVALID_FILE_COMMAND");
        let mut command = QueuedCommand::new(value);
        command.file = Some(file);
        self.enqueue(command)
    }
    fn enqueue(&self, mut command: QueuedCommand) -> anyhow::Result<()> {
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
        self.events.try_recv().ok()
    }
    pub fn wait_poll(&self, timeout: Duration) -> Option<String> {
        self.events.recv_timeout(timeout).ok()
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
        self.events.wake();
    }
    #[cfg(feature = "dlna-live")]
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
    #[cfg(feature = "file-sender")]
    resource_tasks: Vec<JoinHandle<()>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(feature = "file-sender")]
    io: BlockingIo,
    scan: discovery::ScanTask,
    #[cfg(feature = "file-sender")]
    resources: Vec<Arc<Resource>>,
    #[cfg(feature = "dlna")]
    dlna_discovery: discovery::DlnaDiscovery,
    #[cfg(feature = "dlna")]
    dlna_playing: Option<(dlna::Controller, String)>,
    events: Events,
    #[cfg(feature = "file-proxy")]
    bridge: bridge_resource::BridgeResource,
    #[cfg(feature = "dlna-live")]
    live: Arc<StdMutex<Option<Arc<crate::live::LiveResource>>>>,
    #[cfg(feature = "dlna-live")]
    profiles: Option<Arc<StdMutex<crate::profiles::ProfileStore>>>,
    #[cfg(feature = "dlna-live")]
    live_context: Option<LiveContext>,
    #[cfg(feature = "dlna-live")]
    live_task: Option<JoinHandle<()>>,
}
#[cfg(feature = "dlna-live")]
struct LiveContext {
    device: dlna::Renderer,
    profile: String,
    probe: bool,
    url: String,
    generation: u64,
}
impl Runtime {
    async fn complete_discovery(&mut self, events: &Events) {
        #[cfg(feature = "dlna")]
        self.dlna_discovery.complete(events).await;
        #[cfg(not(feature = "dlna"))]
        {
            let _ = events;
            std::future::pending::<()>().await;
        }
    }
    async fn command(
        &mut self,
        mut c: Value,
        granted_file: Option<std::fs::File>,
        command_generation: u64,
    ) -> anyhow::Result<()> {
        if c["op"] != "stop"
            && command_generation != self.generation.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(());
        }
        #[cfg(not(feature = "file-sender"))]
        let _ = granted_file;
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
                let advertisement = if !c["advertise"].as_bool().unwrap_or(true) {
                    // Apple hosts publish with system Bonjour; avoid raw multicast entitlements.
                    None
                } else if let IpAddr::V4(ip) = address.ip() {
                    Some(discovery::Advertisement::start(
                        &self.identity.id.to_string(),
                        c["name"].as_str().unwrap_or("LanCast"),
                        ip,
                        address.port(),
                        server.variant(),
                        &self.identity.fingerprint,
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
                let generation = c["scanGeneration"]
                    .as_u64()
                    .context("SCAN_GENERATION_REQUIRED")?;
                self.scan.start(self.events.clone(), generation).await;
            }
            "connect" => {
                ensure!(
                    self.client.as_ref().is_none_or(Client::is_closed),
                    "ALREADY_CONNECTED"
                );
                if let Some(client) = &mut self.client {
                    client.close().await;
                }
                self.client = None;
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
                let msg = Message::from_value(c["message"].take())?;
                if let Some(client) = &self.client {
                    client.send(msg)?;
                } else if let Some(server) = &self.server {
                    server.send(msg).await?;
                } else {
                    anyhow::bail!("NOT_CONNECTED");
                }
            }
            "stop" => {
                self.scan.stop().await;
                #[cfg(feature = "dlna")]
                self.dlna_discovery.stop().await;
                #[cfg(feature = "file-proxy")]
                self.bridge.close(None).await;
                #[cfg(feature = "dlna-live")]
                if let Some(live) = self.live.lock().unwrap().take() {
                    live.revoke();
                }
                #[cfg(feature = "dlna-live")]
                {
                    self.live_context = None;
                    if let Some(task) = self.live_task.take() {
                        let _ = task.await;
                    }
                }
                if let Some(client) = &mut self.client {
                    client.close().await;
                }
                self.client = None;
                #[cfg(feature = "file-sender")]
                for resource in self.resources.drain(..) {
                    resource.revoke();
                }
                #[cfg(feature = "file-sender")]
                for mut task in self.resource_tasks.drain(..) {
                    if tokio::time::timeout(Duration::from_secs(2), &mut task)
                        .await
                        .is_err()
                    {
                        task.abort();
                        let _ = task.await;
                    }
                }
                #[cfg(feature = "dlna")]
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
            #[cfg(feature = "dlna")]
            "dlna.scan" => {
                let generation = c["scanGeneration"]
                    .as_u64()
                    .context("SCAN_GENERATION_REQUIRED")?;
                let interface = string(&c, "interface")?.parse()?;
                self.dlna_discovery.start(interface, generation).await;
            }
            #[cfg(feature = "file-sender")]
            "file.share" => {
                ensure!(self.resources.is_empty(), "STOP_CURRENT_FILE_FIRST");
                let allowed_ip: IpAddr = string(&c, "allowedIp")?.parse()?;
                let path = c["path"].as_str().map(str::to_owned);
                let resource = self
                    .io
                    .run(command_generation, move |guard| {
                        let file = if let Some(file) = granted_file {
                            file
                        } else if let Some(path) = path {
                            std::fs::File::open(path)?
                        } else {
                            anyhow::bail!("FILE_REQUIRED")
                        };
                        guard.check()?;
                        let resource = Resource::from_file(file, allowed_ip)?;
                        guard.check()?;
                        Ok(resource)
                    })
                    .await?;
                if command_generation != self.generation.load(std::sync::atomic::Ordering::Acquire)
                {
                    return Ok(());
                }
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
                self.resource_tasks.push(task);
            }
            #[cfg(feature = "dlna")]
            "dlna.load" | "dlna.command" => {
                let device_id = string(&c, "deviceId")?;
                let controller = if let Some((controller, _)) = &self.dlna_playing
                    && controller.device.id == device_id
                {
                    controller.clone()
                } else {
                    let device = self
                        .dlna_discovery
                        .get(device_id)
                        .context("DLNA_UNAVAILABLE")?;
                    dlna::Controller::new(device.clone())?
                };
                #[cfg(feature = "dlna-live")]
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
                        json!({"deviceId":device_id,"action":"load","response":{"kind":"accepted"},"firstFrameMeasured":false}),
                    );
                } else {
                    let action = string(&c, "action")?;
                    let response = match controller.command(action, c["positionMs"].as_u64()).await
                    {
                        Ok(response) => response,
                        Err(error) if matches!(action, "state" | "position") => {
                            event(
                                &self.events,
                                "dlna.state",
                                json!({"deviceId":device_id,"action":action,"error":error.to_string(),"firstFrameMeasured":false}),
                            );
                            return Ok(());
                        }
                        Err(error) => return Err(error),
                    };
                    event(
                        &self.events,
                        "dlna.state",
                        json!({"deviceId":device_id,"action":c["action"],"response":response,"firstFrameMeasured":false}),
                    );
                }
            }
            #[cfg(feature = "file-proxy")]
            "bridge.create" => {
                let id = string(&c, "requestId")?;
                let url = self
                    .bridge
                    .open(
                        id.to_owned(),
                        string(&c, "url")?,
                        string(&c, "fingerprint")?,
                    )
                    .await?;
                event(
                    &self.events,
                    "bridge.ready",
                    json!({"url":url,"requestId":id}),
                );
            }
            #[cfg(feature = "file-proxy")]
            "bridge.close" => {
                self.bridge.close(Some(string(&c, "requestId")?)).await;
            }
            #[cfg(feature = "dlna-live")]
            "profiles.open" | "profile.check" | "probe.confirm" | "live.create" => {
                self.live_command(c, command_generation).await?;
            }
            _ => anyhow::bail!("UNKNOWN_COMMAND"),
        }
        Ok(())
    }
}
