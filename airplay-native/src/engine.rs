use crate::{avc::AvcConfig, identity::Identity};
use rairplay::{
    config::{self, Features, SessionObserver},
    playback::{
        self, ChannelHandle,
        audio::{AudioDevice, AudioPacket, AudioParams, CodecKind},
        video::{PacketKind, VideoDevice, VideoPacket, VideoParams},
    },
};
use std::{
    collections::HashMap,
    fmt, io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub enum Event {
    Ready {
        port: u16,
        name: String,
        device_id: String,
        public_key: String,
        features: u64,
    },
    Request {
        session: u64,
        name: String,
        peer: String,
    },
    Closed {
        session: u64,
        reason: String,
    },
    Error(String),
    VideoConfig {
        session: u64,
        config: AvcConfig,
    },
    Video {
        session: u64,
        pts: i64,
        key: bool,
        data: Vec<u8>,
    },
    AudioConfig {
        session: u64,
        codec: u8,
        rate: u32,
        channels: u8,
        spf: u32,
    },
    Audio {
        session: u64,
        pts: i64,
        data: Vec<u8>,
    },
}
pub trait Output: Send + Sync + 'static {
    fn emit(&self, event: Event) -> bool;
}
struct Connection {
    cancel: CancellationToken,
    approval: Option<oneshot::Sender<bool>>,
    authorized: bool,
    video: bool,
    audio: bool,
    reason: String,
}
pub struct Host {
    output: Arc<dyn Output>,
    connections: Mutex<HashMap<u64, Connection>>,
}
impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AirPlayHost")
    }
}
impl Host {
    pub fn new(output: Arc<dyn Output>) -> Arc<Self> {
        Arc::new(Self {
            output,
            connections: Mutex::new(HashMap::new()),
        })
    }
    pub fn approve(&self, id: u64, accept: bool) {
        let mut all = self.connections.lock().unwrap();
        if let Some(c) = all.get_mut(&id)
            && let Some(tx) = c.approval.take()
        {
            c.authorized = accept;
            let _ = tx.send(accept);
        }
    }
    pub fn stop_session(&self, id: u64, reason: &str) {
        if let Some(c) = self.connections.lock().unwrap().get_mut(&id) {
            if c.reason == "DISCONNECTED" {
                c.reason = reason.into();
            }
            c.cancel.cancel();
        }
    }
    pub fn stop_all(&self) {
        for c in self.connections.lock().unwrap().values() {
            c.cancel.cancel();
        }
    }
    fn claim(&self, id: u64, video: bool) -> io::Result<()> {
        let mut all = self.connections.lock().unwrap();
        let c = all
            .get_mut(&id)
            .ok_or_else(|| io::Error::other("session expired"))?;
        if !c.authorized || c.cancel.is_cancelled() {
            return Err(io::Error::other("not authorized"));
        }
        let flag = if video { &mut c.video } else { &mut c.audio };
        if *flag {
            return Err(io::Error::other("duplicate media stream"));
        }
        *flag = true;
        Ok(())
    }
    fn emit(&self, event: Event, id: u64) {
        if !self.output.emit(event) {
            self.stop_session(id, "MEDIA_BACKPRESSURE");
        }
    }
}
impl SessionObserver for Host {
    fn opened(&self, id: u64, cancel: CancellationToken) {
        self.connections.lock().unwrap().insert(
            id,
            Connection {
                cancel,
                approval: None,
                authorized: false,
                video: false,
                audio: false,
                reason: "DISCONNECTED".into(),
            },
        );
    }
    fn authorize(
        &self,
        id: u64,
        name: String,
        peer: SocketAddr,
    ) -> futures::future::BoxFuture<'static, bool> {
        let (tx, rx) = oneshot::channel();
        if let Some(c) = self.connections.lock().unwrap().get_mut(&id) {
            c.approval = Some(tx);
        } else {
            return Box::pin(async { false });
        }
        self.output.emit(Event::Request {
            session: id,
            name,
            peer: peer.ip().to_string(),
        });
        Box::pin(async move { rx.await.unwrap_or(false) })
    }
    fn closed(&self, id: u64) {
        let old = self.connections.lock().unwrap().remove(&id);
        if let Some(c) = old {
            self.output.emit(Event::Closed {
                session: id,
                reason: c.reason,
            });
        }
    }
}

#[derive(Clone, Debug)]
struct VideoBackend(Arc<Host>);
impl VideoDevice for VideoBackend {}
impl playback::Device for VideoBackend {
    type Params = VideoParams;
    type Stream = VideoSink;
    type Error = io::Error;
    async fn create(
        &self,
        _id: u64,
        params: VideoParams,
        handle: Weak<dyn ChannelHandle>,
    ) -> io::Result<VideoSink> {
        self.0.claim(params.connection_id, true)?;
        Ok(VideoSink {
            host: self.0.clone(),
            session: params.connection_id,
            config: Mutex::new(None),
            handle,
            started: std::time::Instant::now(),
        })
    }
}
struct VideoSink {
    host: Arc<Host>,
    session: u64,
    config: Mutex<Option<AvcConfig>>,
    handle: Weak<dyn ChannelHandle>,
    started: std::time::Instant,
}
impl playback::Stream for VideoSink {
    type Content = VideoPacket;
    fn on_data(&self, p: VideoPacket) {
        match p.kind {
            PacketKind::AvcC(_) => match AvcConfig::parse(&p.payload) {
                Ok(config) => {
                    *self.config.lock().unwrap() = Some(config.clone());
                    self.host.emit(
                        Event::VideoConfig {
                            session: self.session,
                            config,
                        },
                        self.session,
                    );
                }
                Err(_) => self.host.stop_session(self.session, "INVALID_VIDEO_CONFIG"),
            },
            PacketKind::Payload => {
                let Some(pts) = p.presentation_us else {
                    if self.started.elapsed() > Duration::from_secs(5) {
                        self.host.stop_session(self.session, "TIMING_UNAVAILABLE");
                    }
                    return;
                };
                let lock = self.config.lock().unwrap();
                let Some(config) = lock.as_ref() else {
                    return;
                };
                match config.annex_b(&p.payload) {
                    Ok((data, key)) => self.host.emit(
                        Event::Video {
                            session: self.session,
                            pts,
                            key,
                            data,
                        },
                        self.session,
                    ),
                    Err(_) => self.host.stop_session(self.session, "INVALID_VIDEO_FRAME"),
                }
            }
            PacketKind::Hvc1(_) => self.host.stop_session(self.session, "HEVC_UNSUPPORTED"),
            _ => {}
        }
    }
    fn on_ok(self) {}
    fn on_err(self, _err: Box<dyn std::error::Error>) {
        self.host
            .stop_session(self.session, "VIDEO_TRANSPORT_FAILED");
    }
}
impl Drop for VideoSink {
    fn drop(&mut self) {
        if let Some(h) = self.handle.upgrade() {
            h.close();
        }
        self.host.stop_session(self.session, "VIDEO_ENDED");
    }
}

#[derive(Clone, Debug)]
struct AudioBackend(Arc<Host>);
impl AudioDevice for AudioBackend {
    fn get_volume(&self) -> f32 {
        0.0
    }
    fn set_volume(&self, _value: f32) {}
}
impl playback::Device for AudioBackend {
    type Params = AudioParams;
    type Stream = AudioSink;
    type Error = io::Error;
    async fn create(
        &self,
        _id: u64,
        p: AudioParams,
        handle: Weak<dyn ChannelHandle>,
    ) -> io::Result<AudioSink> {
        let codec = match p.codec.kind {
            CodecKind::Aac => 1,
            CodecKind::AacEld => 2,
            CodecKind::Pcm if p.codec.bits_per_sample == 16 => 3,
            _ => return Err(io::Error::other("audio codec unsupported")),
        };
        if !matches!(p.codec.sample_rate, 44100 | 48000)
            || p.codec.channels != 2
            || !matches!(p.samples_per_frame, 352 | 480 | 512 | 1024)
        {
            return Err(io::Error::other("audio format unsupported"));
        }
        self.0.claim(p.connection_id, false)?;
        if !self.0.output.emit(Event::AudioConfig {
            session: p.connection_id,
            codec,
            rate: p.codec.sample_rate,
            channels: p.codec.channels,
            spf: p.samples_per_frame,
        }) {
            return Err(io::Error::other("audio decoder unavailable"));
        }
        Ok(AudioSink {
            host: self.0.clone(),
            session: p.connection_id,
            pcm: codec == 3,
            sequence: Mutex::new(None),
            handle,
        })
    }
}
struct AudioSink {
    host: Arc<Host>,
    session: u64,
    pcm: bool,
    sequence: Mutex<Option<u16>>,
    handle: Weak<dyn ChannelHandle>,
}
impl playback::Stream for AudioSink {
    type Content = AudioPacket;
    fn on_data(&self, p: AudioPacket) {
        let Some(pts) = p.presentation_us else {
            return;
        };
        if p.rtp.len() <= 12 {
            return;
        }
        let seq = u16::from_be_bytes(p.rtp[2..4].try_into().unwrap());
        let mut last = self.sequence.lock().unwrap();
        if last.is_some_and(|old| seq.wrapping_sub(old) as i16 <= 0) {
            return;
        }
        *last = Some(seq);
        let mut data = p.rtp[12..].to_vec();
        if data == [0, 0x68, 0x34, 0] {
            return;
        }
        if self.pcm {
            for pair in data.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        self.host.emit(
            Event::Audio {
                session: self.session,
                pts,
                data,
            },
            self.session,
        );
    }
    fn on_ok(self) {}
    fn on_err(self, _err: Box<dyn std::error::Error>) {
        self.host
            .stop_session(self.session, "AUDIO_TRANSPORT_FAILED");
    }
}
impl Drop for AudioSink {
    fn drop(&mut self) {
        if let Some(h) = self.handle.upgrade() {
            h.close();
        }
    }
}

pub fn features() -> Features {
    Features::ScreenMirroring
        | Features::ScreenRotate
        | Features::AirPlayAudio
        | Features::ReceiveAudioPCM
        | Features::ReceiveAudioAAC_LC
        | Features::MFiSoft_FairPlay
        | Features::LegacyPairing
        | Features::UnifiedAdvertisingInfo
        | Features::NTPClock
        | Features::HomeKitPairing
        | Features::ControlChannelEncrypt
}
pub struct Running {
    pub host: Arc<Host>,
    cancel: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Running {
    pub fn start(
        address: Ipv4Addr,
        name: String,
        seed: [u8; 32],
        pin: [u8; 8],
        peer_store: Option<std::path::PathBuf>,
        output: Arc<dyn Output>,
    ) -> io::Result<Self> {
        let pin =
            config::PinCode::try_from(pin).map_err(|_| io::Error::other("invalid pairing PIN"))?;
        let cancel = CancellationToken::new();
        let host = Host::new(output);
        let runner = host.clone();
        let signal = cancel.clone();
        let thread=std::thread::Builder::new().name("lancast-airplay".into()).spawn(move||{
            let result=(||->io::Result<()>{
                let runtime=tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
                let outcome=runtime.block_on(async{
                    let identity=Identity::load(seed,peer_store)?;let device_id=identity.device_id().to_owned();let public_key=identity.public_hex();
                    let config=Arc::new(config::Config{observer:Some(runner.clone()),mac_addr:device_id.parse().map_err(|_|io::Error::other("device identity"))?,name:name.clone(),model:"LanCast".into(),manufacturer:"LanCast contributors".into(),fw_version:env!("CARGO_PKG_VERSION").into(),features:features(),pin:Some(pin),keychain:identity,pairing:config::Pairing::Automatic,
                        audio:config::Audio{buf_size:256*1024,device:AudioBackend(runner.clone())},
                        video:config::Video{width:1920,height:1080,fps:30,buf_size:2*1024*1024,device:VideoBackend(runner.clone())}});
                    let listener=rairplay::transport::DualStackListenerWithRtspRemap::bind(SocketAddrV4::new(address,0),SocketAddrV6::new(Ipv6Addr::UNSPECIFIED,0,0,0))?;
                    runner.output.emit(Event::Ready{port:listener.port(),name,device_id,public_key,features:features().bits()});
                    tokio::select!{_ = signal.cancelled()=>{},result=axum::serve(listener,rairplay::ServiceFactory::new(config))=>{result?;}}
                    runner.stop_all();Ok(())
                });
                runtime.shutdown_timeout(Duration::from_secs(2));outcome
            })();
            if result.is_err(){runner.output.emit(Event::Error("ENGINE_START_OR_IO_FAILED".into()));}
        })?;
        Ok(Self {
            host,
            cancel,
            thread: Some(thread),
        })
    }
    pub fn stop(mut self) {
        self.cancel.cancel();
        self.host.stop_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.host.stop_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
