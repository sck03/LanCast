//! Real synthetic H.264/AAC -> production LiveResource -> mock UPnP TV -> decoded CI artifact.
use anyhow::{Context, ensure};
use bytes::Bytes;
use cast_adapters::{
    dlna::{Controller, Renderer, Service},
    live::LiveResource,
    live_session,
};
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::TcpListener};

#[derive(Default)]
struct Tv {
    uri: String,
    playing: bool,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "usage: validate_live fixture.ts output-directory"
    );
    let fixture = Arc::new(std::fs::read(&args[1])?);
    ensure!(fixture.len().is_multiple_of(188), "unaligned fixture");
    std::fs::create_dir_all(&args[2])?;
    // Select a local LAN address without sending any packet to another machine. Production
    // URL validation remains enabled; the test does not weaken the loopback/SSRF restriction.
    let route = std::net::UdpSocket::bind("0.0.0.0:0")?;
    route.connect("192.0.2.1:9")?;
    let ip = route.local_addr()?.ip();
    ensure!(
        cast_adapters::dlna::lan_ip(ip),
        "test requires a private local interface"
    );
    for (index, version) in [reqwest::Version::HTTP_10, reqwest::Version::HTTP_11]
        .into_iter()
        .enumerate()
    {
        let live = LiveResource::new(ip);
        let (url, http) = live.clone().bind((ip, 0).into()).await?;
        for chunk in fixture.chunks(188 * 348) {
            live.write(chunk)?;
        }
        ensure!(live.ready(), "real sample has no random access point");
        let listener = TcpListener::bind((ip, 0)).await?;
        let endpoint = listener.local_addr()?;
        let tv = Arc::new(Mutex::new(Tv::default()));
        let playing = Arc::new(AtomicBool::new(false));
        let server_tv = tv.clone();
        let server_playing = playing.clone();
        let soap = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let tv = server_tv.clone();
                let playing = server_playing.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |r: Request<Incoming>| {
                        let tv = tv.clone();
                        let playing = playing.clone();
                        async move {
                            let action = r
                                .headers()
                                .get("soapaction")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_owned();
                            let raw = r.into_body().collect().await.unwrap().to_bytes();
                            let raw = String::from_utf8(raw.to_vec()).unwrap();
                            let doc = roxmltree::Document::parse(&raw).unwrap();
                            let mut tv = tv.lock().unwrap();
                            let body = if action.contains("SetAVTransportURI") {
                                tv.uri = doc
                                    .descendants()
                                    .find(|n| n.has_tag_name("CurrentURI"))
                                    .and_then(|n| n.text())
                                    .unwrap()
                                    .to_owned();
                                "<SetAVTransportURIResponse/>".into()
                            } else if action.contains("#Play") {
                                tv.playing = true;
                                playing.store(true, Ordering::Release);
                                "<PlayResponse/>".into()
                            } else if action.contains("#Stop") {
                                tv.playing = false;
                                "<StopResponse/>".into()
                            } else if action.contains("GetTransportInfo") {
                                format!(
                                    "<GetTransportInfoResponse><CurrentTransportState>{}</CurrentTransportState></GetTransportInfoResponse>",
                                    if tv.playing { "PLAYING" } else { "STOPPED" }
                                )
                            } else {
                                format!(
                                    "<GetMediaInfoResponse><CurrentURI>{}</CurrentURI></GetMediaInfoResponse>",
                                    cast_adapters::dlna::escape(&tv.uri)
                                )
                            };
                            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(format!(
                                "<Envelope><Body>{body}</Body></Envelope>"
                            )))))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        let renderer = Renderer {
            id: "test-tv".into(),
            name: "Mock TV".into(),
            ip,
            transport: Service {
                kind: "urn:schemas-upnp-org:service:AVTransport:1".into(),
                control: format!("http://{endpoint}/control"),
            },
            rendering: None,
            connection: None,
            signature: "test".into(),
        };
        let started = Arc::new(AtomicBool::new(false));
        let observed = started.clone();
        let resource = live.clone();
        let media_url = url.clone();
        let supervisor = tokio::spawn(async move {
            live_session::run(
                Controller::new(renderer).unwrap(),
                resource,
                media_url,
                false,
                |kind, body| {
                    if kind == "live.state" && body["state"] == "pulling" {
                        observed.store(true, Ordering::Release);
                    }
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !playing.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let client = reqwest::Client::builder().no_proxy().http1_only().build()?;
        let head = client.head(&url).version(version).send().await?;
        ensure!(
            !head.headers().contains_key("content-length"),
            "finite live HEAD"
        );
        ensure!(live.pulls() == 0, "HEAD counted as a media pull");
        let response = client
            .get(&url)
            .version(version)
            .header("Range", "bytes=0-")
            .send()
            .await?;
        ensure!(
            response.status() == 200,
            "live Range must fall back to continuous 200"
        );
        ensure!(
            !response.headers().contains_key("content-length"),
            "finite live GET"
        );
        ensure!(
            response.headers().contains_key("transfer-encoding")
                == (version == reqwest::Version::HTTP_11),
            "HTTP framing mismatch"
        );
        let output = format!("{}/http-{index}.ts", args[2]);
        let reader = tokio::spawn(async move {
            let mut file = tokio::fs::File::create(output).await.unwrap();
            let mut bytes = 0;
            let mut stream = response.bytes_stream();
            while let Some(Ok(chunk)) = stream.next().await {
                bytes += chunk.len();
                file.write_all(&chunk).await.unwrap();
            }
            file.flush().await.unwrap();
            bytes
        });
        for block in fixture.chunks(188 * 7) {
            live.write(block)?;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while !started.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        // The TV owner may switch sources while this session is stopping. Do not stop their source.
        if index == 1 {
            tv.lock().unwrap().uri = "http://192.0.2.5/other.mp4".into();
        }
        live.revoke();
        ensure!(
            tokio::time::timeout(Duration::from_secs(3), reader).await?? > 188 * 20,
            "no live media received"
        );
        tokio::time::timeout(Duration::from_secs(3), supervisor).await???;
        ensure!(
            tv.lock().unwrap().playing == (index == 1),
            "incorrect renderer ownership on cancellation"
        );
        http.await?;
        soap.abort();
        println!(
            "PASS HTTP version={version:?}, TS bytes={}, confirmed pull, cancellation, SOAP stop",
            live.delivered()
        );
        ensure!(
            live.write(&fixture[..188]).is_err(),
            "revoked stream accepted data"
        );
        client
            .get(url)
            .send()
            .await
            .err()
            .context("revoked endpoint still accepts connections")?;
    }
    Ok(())
}
