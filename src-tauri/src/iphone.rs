//! iPhone (or any phone with a browser) as a webcam, nothing to install on the phone:
//! Safari opens a page served over HTTPS on the LAN (self-signed certificate), captures the
//! camera with getUserMedia and streams JPEG frames back over a WebSocket.
//! The camera module consumes the frames through `set_sink`.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

use crate::util::{data_dir, err, Res};

static PAGE: &str = include_str!("iphone.html");
const PORTS: [u16; 4] = [8443, 8444, 8445, 8446];

/// Receives every JPEG frame from the phone.
pub type Sink = Box<dyn FnMut(&[u8]) + Send>;

#[derive(Serialize, Clone, Default)]
pub struct Phone {
    pub name: String,
    pub os: String,
    pub camera: bool,
    pub width: u32,
    pub height: u32,
    pub facing: String,
    pub torch: bool,
}

#[derive(Serialize, Clone)]
pub struct Status {
    pub running: bool,
    pub urls: Vec<String>,
    /// QR code (SVG) for each of `urls`.
    pub qrs: Vec<String>,
    pub phone: Option<Phone>,
}

struct Server {
    /// None in tests.
    app: Option<AppHandle>,
    stop: AtomicBool,
    urls: Vec<String>,
    qrs: Vec<String>,
    token: String,
    next_id: AtomicU64,
    /// Current phone connection: id, info, commands to send.
    phone: Mutex<Option<(u64, Phone, Sender<String>)>>,
    sink: Mutex<Option<Sink>>,
}

static SERVER: Mutex<Option<Arc<Server>>> = Mutex::new(None);

fn current() -> Option<Arc<Server>> {
    SERVER.lock().unwrap().clone()
}

pub fn status() -> Status {
    match current() {
        Some(s) => Status {
            running: true,
            urls: s.urls.clone(),
            qrs: s.qrs.clone(),
            phone: s.phone.lock().unwrap().as_ref().map(|p| p.1.clone()),
        },
        None => Status { running: false, urls: Vec::new(), qrs: Vec::new(), phone: None },
    }
}

fn emit(s: &Server) {
    if let Some(app) = &s.app {
        let _ = app.emit("iphone-status", status());
    }
}

pub fn connected() -> bool {
    current().map(|s| s.phone.lock().unwrap().is_some()).unwrap_or(false)
}

pub fn set_sink(sink: Option<Sink>) {
    if let Some(s) = current() {
        *s.sink.lock().unwrap() = sink;
    }
}

/// Sends a command to the phone page (JSON).
pub fn send(cmd: Value) {
    if let Some(s) = current() {
        if let Some((_, _, tx)) = s.phone.lock().unwrap().as_ref() {
            let _ = tx.send(cmd.to_string());
        }
    }
}

pub fn stop() {
    if let Some(s) = SERVER.lock().unwrap().take() {
        s.stop.store(true, Ordering::SeqCst);
        *s.sink.lock().unwrap() = None;
        emit(&s);
    }
}

pub fn start(app: Option<AppHandle>) -> Res<Status> {
    if current().is_some() {
        return Ok(status());
    }
    let ips = local_ips();
    if ips.is_empty() {
        return Err("Нет сети: подключите ПК и iPhone к одному Wi‑Fi".into());
    }
    let (listener, port) = PORTS
        .iter()
        .find_map(|&p| TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).ok().map(|l| (l, p)))
        .ok_or("Порты 8443–8446 заняты")?;
    let token = token()?;
    let config = Arc::new(tls_config(&ips)?);
    let urls: Vec<String> = ips.iter().map(|ip| format!("https://{ip}:{port}/?k={token}")).collect();
    let qrs = urls
        .iter()
        .map(|u| {
            qrcode::QrCode::new(u.as_bytes()).map(|code| {
                code.render::<qrcode::render::svg::Color>()
                    .min_dimensions(200, 200)
                    .dark_color(qrcode::render::svg::Color("#000000"))
                    .light_color(qrcode::render::svg::Color("#ffffff"))
                    .build()
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    let server = Arc::new(Server {
        app,
        stop: AtomicBool::new(false),
        urls,
        qrs,
        token,
        next_id: AtomicU64::new(1),
        phone: Mutex::new(None),
        sink: Mutex::new(None),
    });
    *SERVER.lock().unwrap() = Some(server.clone());

    listener.set_nonblocking(true).map_err(err)?;
    let s = server.clone();
    std::thread::spawn(move || {
        while !s.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((tcp, _)) => {
                    let (s, config) = (s.clone(), config.clone());
                    std::thread::spawn(move || {
                        let _ = handle(&s, config, tcp);
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    });
    Ok(status())
}

// ── network, certificate, token ──

/// Private IPv4 addresses of this PC, most likely reachable from the phone first:
/// iPhone Personal Hotspot, then real adapters, VPN and virtual adapters last.
fn local_ips() -> Vec<IpAddr> {
    const VIRTUAL: [&str; 14] = [
        "tun", "tap", "vpn", "wg", "wireguard", "vethernet", "virtual", "vmware", "vbox", "docker", "hyper-v",
        "zerotier", "tailscale", "loopback",
    ];
    let mut found: Vec<(u8, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| {
            let IpAddr::V4(v4) = i.ip() else { return None };
            if !v4.is_private() {
                return None;
            }
            let name = i.name.to_ascii_lowercase();
            let o = v4.octets();
            let rank = if o[0] == 172 && o[1] == 20 && o[2] == 10 {
                0 // iPhone Personal Hotspot (Wi‑Fi or USB)
            } else if VIRTUAL.iter().any(|v| name.contains(v)) {
                4
            } else if o[0] == 192 {
                1
            } else if o[0] == 10 {
                2
            } else {
                3
            };
            Some((rank, i.ip()))
        })
        .collect();
    found.sort();
    found.dedup_by_key(|x| x.1);
    found.into_iter().map(|(_, ip)| ip).collect()
}

fn token() -> Res<String> {
    let path = data_dir().join("iphone-token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        if t.trim().len() == 12 {
            return Ok(t.trim().to_string());
        }
    }
    let t: String = (0..12).map(|_| char::from_digit(rand::random::<u32>() % 36, 36).unwrap()).collect();
    std::fs::create_dir_all(data_dir()).map_err(err)?;
    std::fs::write(&path, &t).map_err(err)?;
    Ok(t)
}

/// A self-signed certificate for the PC's addresses, kept while the addresses stay the same,
/// so Safari asks to trust it only once.
fn tls_config(ips: &[IpAddr]) -> Res<ServerConfig> {
    let dir = data_dir().join("iphone");
    let (cert_p, key_p, sans_p) = (dir.join("cert.der"), dir.join("key.der"), dir.join("names.txt"));
    let mut names: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    names.push("localhost".into());
    let saved = std::fs::read_to_string(&sans_p).unwrap_or_default();
    let fresh = names.iter().all(|n| saved.lines().any(|l| l == n));
    let (cert, key) = match (fresh, std::fs::read(&cert_p), std::fs::read(&key_p)) {
        (true, Ok(c), Ok(k)) => (c, k),
        _ => {
            let ck = rcgen::generate_simple_self_signed(names.clone()).map_err(err)?;
            let (c, k) = (ck.cert.der().to_vec(), ck.key_pair.serialize_der());
            std::fs::create_dir_all(&dir).map_err(err)?;
            let _ = std::fs::write(&cert_p, &c);
            let _ = std::fs::write(&key_p, &k);
            let _ = std::fs::write(&sans_p, names.join("\n"));
            (c, k)
        }
    };
    ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert)], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
        .map_err(err)
}

// ── HTTP + WebSocket ──

type Tls = StreamOwned<ServerConnection, TcpStream>;

fn handle(s: &Arc<Server>, config: Arc<ServerConfig>, tcp: TcpStream) -> Res<()> {
    tcp.set_nonblocking(false).map_err(err)?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).map_err(err)?;
    tcp.set_nodelay(true).ok();
    let mut tls = StreamOwned::new(ServerConnection::new(config).map_err(err)?, tcp);

    // Request head.
    let mut head = Vec::new();
    let mut byte = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = tls.read(&mut byte).map_err(err)?;
        if n == 0 || head.len() > 16 * 1024 {
            return Ok(());
        }
        head.extend_from_slice(&byte[..n]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let target = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let authorized = query.split('&').any(|kv| kv == format!("k={}", s.token));

    let reply = |tls: &mut Tls, status: &str, kind: &str, body: &[u8]| -> Res<()> {
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            body.len()
        );
        tls.write_all(head.as_bytes()).map_err(err)?;
        tls.write_all(body).map_err(err)?;
        tls.flush().map_err(err)?;
        tls.conn.send_close_notify();
        let _ = tls.flush();
        Ok(())
    };

    match path {
        "/" if authorized => reply(&mut tls, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        "/ws" if authorized && headers.get("upgrade").map(|u| u.eq_ignore_ascii_case("websocket")).unwrap_or(false) => {
            let key = headers.get("sec-websocket-key").ok_or("no key")?;
            let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            tls.write_all(resp.as_bytes()).map_err(err)?;
            tls.flush().map_err(err)?;
            let ua = headers.get("user-agent").cloned().unwrap_or_default();
            session(s, WebSocket::from_raw_socket(tls, Role::Server, None), &ua)
        }
        "/" | "/ws" => reply(&mut tls, "403 Forbidden", "text/plain; charset=utf-8", "Android Tools: откройте ссылку из программы заново".as_bytes()),
        _ => reply(&mut tls, "404 Not Found", "text/plain", b""),
    }
}

/// Phone name and OS version from the Safari user agent.
fn describe(ua: &str) -> (String, String) {
    let ios = ua
        .split("OS ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .filter(|v| v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .map(|v| v.replace('_', "."));
    for kind in ["iPhone", "iPad"] {
        if ua.contains(kind) {
            return (kind.to_string(), ios.map(|v| format!("iOS {v}")).unwrap_or_default());
        }
    }
    if let Some(v) = ua.split("Android ").nth(1).and_then(|r| r.split([';', ')']).next()) {
        return ("Android".into(), format!("Android {v}"));
    }
    ("Телефон".into(), String::new())
}

fn session(s: &Arc<Server>, mut ws: WebSocket<Tls>, ua: &str) -> Res<()> {
    let id = s.next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    let (name, os) = describe(ua);
    // A new connection (page reload, second phone) replaces the previous one.
    *s.phone.lock().unwrap() = Some((id, Phone { name, os, ..Default::default() }, tx));
    emit(s);
    ws.get_mut().sock.set_read_timeout(Some(Duration::from_millis(25))).map_err(err)?;

    let mine = |s: &Server| s.phone.lock().unwrap().as_ref().map(|p| p.0 == id).unwrap_or(false);
    let result = loop {
        if s.stop.load(Ordering::SeqCst) || !mine(s) {
            let _ = ws.close(None);
            let _ = ws.flush();
            break Ok(());
        }
        match ws.read() {
            Ok(Message::Binary(frame)) => {
                if let Some(sink) = s.sink.lock().unwrap().as_mut() {
                    sink(&frame);
                }
            }
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                    if v["type"] == "state" {
                        if let Some(p) = s.phone.lock().unwrap().as_mut().filter(|p| p.0 == id) {
                            p.1.camera = v["camera"].as_bool().unwrap_or(false);
                            p.1.width = v["width"].as_u64().unwrap_or(0) as u32;
                            p.1.height = v["height"].as_u64().unwrap_or(0) as u32;
                            p.1.facing = v["facing"].as_str().unwrap_or("").to_string();
                            p.1.torch = v["torch"].as_bool().unwrap_or(false);
                        }
                        emit(s);
                    } else if v["type"] == "error" {
                        if let Some(app) = &s.app {
                            let _ = app.emit("camera-error", format!("iPhone: {}", v["text"].as_str().unwrap_or("")));
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => break Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => break Err(e.to_string()),
        }
        while let Ok(cmd) = rx.try_recv() {
            if ws.send(Message::text(cmd)).is_err() {
                break;
            }
        }
    };
    {
        let mut p = s.phone.lock().unwrap();
        if p.as_ref().map(|p| p.0 == id).unwrap_or(false) {
            *p = None;
        }
    }
    emit(s);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agents() {
        let ios = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
        assert_eq!(describe(ios), ("iPhone".into(), "iOS 17.5.1".into()));
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/126.0 Mobile Safari/537.36";
        assert_eq!(describe(android), ("Android".into(), "Android 14".into()));
    }

    /// A Python client plays the phone: loads the page, opens the WebSocket, sends JPEG frames.
    #[test]
    #[ignore = "opens a LAN port and needs python"]
    fn https_page_and_websocket_frames() {
        use std::sync::atomic::AtomicUsize;
        let st = start(None).expect("server");
        let got = Arc::new(AtomicUsize::new(0));
        let g = got.clone();
        set_sink(Some(Box::new(move |jpeg: &[u8]| {
            assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
            g.fetch_add(1, Ordering::SeqCst);
        })));
        let url = st.urls[0].clone();
        let out = std::process::Command::new("python")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/iphone_client.py"))
            .arg(&url)
            .output()
            .expect("python");
        println!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success());
        std::thread::sleep(Duration::from_millis(300));
        assert!(got.load(Ordering::SeqCst) >= 5, "frames: {}", got.load(Ordering::SeqCst));
        stop();
    }

    #[test]
    fn has_lan_address() {
        println!("{:?}", local_ips());
    }
}
