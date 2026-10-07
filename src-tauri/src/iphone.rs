//! iPhone (or any phone with a browser) as a webcam, nothing to install on the phone:
//! Safari opens a page served over HTTPS on the LAN (self-signed certificate), captures the
//! camera with getUserMedia and uploads JPEG frames with fetch() over keep-alive connections;
//! commands for the phone come back through a long poll. Only plain requests to the page's own
//! origin are used: Safari applies the certificate exception the user accepted to them, while a
//! WebSocket to a server with a self-signed certificate fails on iOS.
//! The camera module consumes the frames through `set_sink`.
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::util::{data_dir, err, Res};

static PAGE: &str = include_str!("iphone.html");
const PORTS: [u16; 4] = [8443, 8444, 8445, 8446];
/// Longest wait of a poll; the page polls again at once.
const POLL: Duration = Duration::from_secs(8);
/// A page not heard from for this long is gone.
const STALE: Duration = Duration::from_secs(14);
const MAX_BODY: usize = 16 << 20;
/// Certificate lifetime: iOS refuses server certificates valid for more than 825 days.
const CERT_DAYS: i64 = 365;

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

/// The phone page talking to the PC.
struct Session {
    sid: String,
    phone: Phone,
    cmds: VecDeque<String>,
    seen: Instant,
}

impl Session {
    fn alive(&self) -> bool {
        self.seen.elapsed() < STALE
    }
}

struct Server {
    /// None in tests.
    app: Option<AppHandle>,
    stop: AtomicBool,
    urls: Vec<String>,
    qrs: Vec<String>,
    token: String,
    /// A newly opened page replaces the previous one.
    session: Mutex<Option<Session>>,
    /// Wakes polls waiting for commands.
    wake: Condvar,
    /// The running camera's start command, replayed to a page that (re)connects.
    wanted: Mutex<Option<String>>,
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
            phone: s.session.lock().unwrap().as_ref().filter(|x| x.alive()).map(|x| x.phone.clone()),
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
    current().map(|s| s.session.lock().unwrap().as_ref().map(Session::alive).unwrap_or(false)).unwrap_or(false)
}

/// Waits up to `timeout` for a phone page; false when `stop` is set first or nobody came.
pub fn wait_connected(stop: &AtomicBool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !stop.load(Ordering::SeqCst) {
        if connected() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

pub fn set_sink(sink: Option<Sink>) {
    if let Some(s) = current() {
        *s.sink.lock().unwrap() = sink;
    }
}

/// Sends a command to the phone page (JSON).
pub fn send(cmd: Value) {
    let Some(s) = current() else { return };
    let text = cmd.to_string();
    match cmd["cmd"].as_str() {
        Some("start") => *s.wanted.lock().unwrap() = Some(text.clone()),
        Some("stop") => *s.wanted.lock().unwrap() = None,
        _ => {}
    }
    if let Some(x) = s.session.lock().unwrap().as_mut() {
        x.cmds.push_back(text);
    }
    s.wake.notify_all();
}

pub fn stop() {
    // Take the server out first: `emit` reads SERVER again.
    let taken = SERVER.lock().unwrap().take();
    if let Some(s) = taken {
        s.stop.store(true, Ordering::SeqCst);
        *s.sink.lock().unwrap() = None;
        s.wake.notify_all();
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
        session: Mutex::new(None),
        wake: Condvar::new(),
        wanted: Mutex::new(None),
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
    // A phone that went away (screen locked, Safari closed) is noticed without a request.
    let s = server.clone();
    std::thread::spawn(move || {
        while !s.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(1));
            let gone = {
                let mut g = s.session.lock().unwrap();
                let stale = g.as_ref().map(|x| !x.alive()).unwrap_or(false);
                if stale {
                    *g = None;
                }
                stale
            };
            if gone {
                emit(&s);
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

fn today() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64 / 86_400).unwrap_or(0)
}

/// Days since 1970-01-01 → (year, month, day).
fn civil(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m as u8, d as u8)
}

/// A self-signed certificate for the PC's addresses, valid for a year and marked for TLS
/// servers as iOS requires. It is kept while the addresses stay the same, so Safari asks to
/// trust it once a year.
fn tls_config(ips: &[IpAddr]) -> Res<ServerConfig> {
    let dir = data_dir().join("iphone");
    let (cert_p, key_p, sans_p) = (dir.join("cert.der"), dir.join("key.der"), dir.join("names.txt"));
    let mut names: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    names.push("localhost".into());
    let saved = std::fs::read_to_string(&sans_p).unwrap_or_default();
    let today = today();
    // First line: the day the certificate expires. Certificates of older versions have none
    // and never expire, which iOS does not accept: they are replaced.
    let expires = saved
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("expires="))
        .and_then(|d| d.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let fresh = expires - today > 30 && names.iter().all(|n| saved.lines().any(|l| l == n));
    let (cert, key) = match (fresh, std::fs::read(&cert_p), std::fs::read(&key_p)) {
        (true, Ok(c), Ok(k)) => (c, k),
        _ => {
            let mut params = rcgen::CertificateParams::new(names.clone()).map_err(err)?;
            let (y, m, d) = civil(today - 1);
            params.not_before = rcgen::date_time_ymd(y, m, d);
            let (y, m, d) = civil(today + CERT_DAYS);
            params.not_after = rcgen::date_time_ymd(y, m, d);
            params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
            let key_pair = rcgen::KeyPair::generate().map_err(err)?;
            let cert = params.self_signed(&key_pair).map_err(err)?;
            let (c, k) = (cert.der().to_vec(), key_pair.serialize_der());
            std::fs::create_dir_all(&dir).map_err(err)?;
            let _ = std::fs::write(&cert_p, &c);
            let _ = std::fs::write(&key_p, &k);
            let _ = std::fs::write(&sans_p, format!("expires={}\n{}", today + CERT_DAYS, names.join("\n")));
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

// ── HTTP ──

type Tls = StreamOwned<ServerConnection, TcpStream>;

struct Request {
    method: String,
    path: String,
    params: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn param(&self, k: &str) -> &str {
        self.params.get(k).map(String::as_str).unwrap_or("")
    }

    fn header(&self, k: &str) -> &str {
        self.headers.get(k).map(String::as_str).unwrap_or("")
    }
}

/// One HTTP/1.1 request; None when the client closed the connection.
fn read_request(r: &mut impl BufRead) -> Res<Option<Request>> {
    let mut line = String::new();
    for _ in 0..4 {
        line.clear();
        if r.read_line(&mut line).map_err(err)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    let mut size = line.len();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).map_err(err)?;
        size += n;
        if n == 0 || size > 32 * 1024 {
            return Ok(None);
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err("chunked body".into());
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(err)?;
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let params = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Ok(Some(Request { method, path: path.to_string(), params, headers, body }))
}

fn reply(tls: &mut Tls, status: &str, kind: &str, body: &[u8], keep: bool) -> Res<()> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: {}\r\n\r\n",
        body.len(),
        if keep { "keep-alive" } else { "close" }
    )
    .into_bytes();
    out.extend_from_slice(body);
    tls.write_all(&out).map_err(err)?;
    tls.flush().map_err(err)
}

/// Serves requests of one keep-alive connection.
fn handle(s: &Arc<Server>, config: Arc<ServerConfig>, tcp: TcpStream) -> Res<()> {
    tcp.set_nonblocking(false).map_err(err)?;
    tcp.set_read_timeout(Some(Duration::from_secs(30))).map_err(err)?;
    tcp.set_write_timeout(Some(Duration::from_secs(15))).map_err(err)?;
    tcp.set_nodelay(true).ok();
    let tls = StreamOwned::new(ServerConnection::new(config).map_err(err)?, tcp);
    let mut conn = BufReader::with_capacity(64 * 1024, tls);
    while !s.stop.load(Ordering::SeqCst) {
        let Some(req) = read_request(&mut conn)? else { break };
        if !respond(s, &req, conn.get_mut())? {
            break;
        }
    }
    let tls = conn.get_mut();
    tls.conn.send_close_notify();
    let _ = tls.flush();
    Ok(())
}

const JSON: &str = "application/json";

/// Returns whether the connection stays open.
fn respond(s: &Arc<Server>, req: &Request, tls: &mut Tls) -> Res<bool> {
    let keep = !req.header("connection").eq_ignore_ascii_case("close");
    if req.param("k") != s.token {
        let known = matches!(req.path.as_str(), "/" | "/poll" | "/frame" | "/state");
        let (code, text) = if known {
            ("403 Forbidden", "Android Tools: откройте ссылку из программы заново")
        } else {
            ("404 Not Found", "")
        };
        reply(tls, code, "text/plain; charset=utf-8", text.as_bytes(), false)?;
        return Ok(false);
    }
    let sid = req.param("sid");
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => reply(tls, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes(), keep)?,
        ("GET", "/poll") => match poll(s, sid, req.header("user-agent"), !req.param("hello").is_empty()) {
            Some(cmds) => reply(tls, "200 OK", JSON, format!("[{}]", cmds.join(",")).as_bytes(), keep)?,
            None => reply(tls, "409 Conflict", JSON, b"[]", keep)?,
        },
        ("POST", "/frame") => {
            if with_session(s, sid, |_| {}) {
                if let Some(sink) = s.sink.lock().unwrap().as_mut() {
                    sink(&req.body);
                }
                reply(tls, "200 OK", JSON, b"{}", keep)?
            } else {
                not_mine(s, tls, keep)?
            }
        }
        ("POST", "/state") => {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let mine = with_session(s, sid, |x| {
                if v["type"] == "state" {
                    x.phone.camera = v["camera"].as_bool().unwrap_or(false);
                    x.phone.width = v["width"].as_u64().unwrap_or(0) as u32;
                    x.phone.height = v["height"].as_u64().unwrap_or(0) as u32;
                    x.phone.facing = v["facing"].as_str().unwrap_or("").to_string();
                    x.phone.torch = v["torch"].as_bool().unwrap_or(false);
                }
            });
            if !mine {
                not_mine(s, tls, keep)?;
            } else {
                if v["type"] == "state" {
                    emit(s);
                } else if v["type"] == "error" {
                    if let Some(app) = &s.app {
                        let _ = app.emit("camera-error", format!("iPhone: {}", v["text"].as_str().unwrap_or("")));
                    }
                }
                reply(tls, "200 OK", JSON, b"{}", keep)?;
            }
        }
        _ => reply(tls, "404 Not Found", "text/plain", b"", keep)?,
    }
    Ok(keep)
}

/// Runs `f` on the session if `sid` is the connected page.
fn with_session(s: &Server, sid: &str, f: impl FnOnce(&mut Session)) -> bool {
    let mut g = s.session.lock().unwrap();
    match g.as_mut().filter(|x| x.sid == sid) {
        Some(x) => {
            x.seen = Instant::now();
            f(x);
            true
        }
        None => false,
    }
}

/// A request from a page that is not the connected one: 409 when another page took over,
/// 410 when its connection timed out (its next poll registers it again).
fn not_mine(s: &Server, tls: &mut Tls, keep: bool) -> Res<()> {
    let taken = s.session.lock().unwrap().as_ref().map(Session::alive).unwrap_or(false);
    reply(tls, if taken { "409 Conflict" } else { "410 Gone" }, JSON, b"{}", keep)
}

/// Long poll of the phone page: registers it and returns the queued commands, waiting up to
/// `POLL` for one. None: another page is connected (`hello` marks a page load: the newest
/// page wins).
fn poll(s: &Server, sid: &str, ua: &str, hello: bool) -> Option<Vec<String>> {
    if sid.is_empty() {
        return None;
    }
    let mut g = s.session.lock().unwrap();
    if g.as_ref().map(|x| x.sid != sid).unwrap_or(true) {
        if !hello && g.as_ref().map(Session::alive).unwrap_or(false) {
            return None;
        }
        let (name, os) = describe(ua);
        let cmds = s.wanted.lock().unwrap().iter().cloned().collect();
        *g = Some(Session { sid: sid.to_string(), phone: Phone { name, os, ..Default::default() }, cmds, seen: Instant::now() });
        drop(g);
        emit(s);
        g = s.session.lock().unwrap();
    }
    let deadline = Instant::now() + POLL;
    loop {
        {
            let x = g.as_mut().filter(|x| x.sid == sid)?;
            x.seen = Instant::now();
            if !x.cmds.is_empty() || s.stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return Some(x.cmds.drain(..).collect());
            }
        }
        let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(500));
        g = s.wake.wait_timeout(g, left).unwrap().0;
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

    #[test]
    fn dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn keep_alive_requests() {
        let raw = b"POST /frame?k=abc&sid=s1 HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nxyz\
                    GET /poll?k=abc&sid=s1&hello=1 HTTP/1.1\r\nUser-Agent: iPhone\r\nConnection: close\r\n\r\n";
        let mut r = std::io::Cursor::new(&raw[..]);
        let a = read_request(&mut r).unwrap().unwrap();
        assert_eq!((a.method.as_str(), a.path.as_str(), a.param("sid"), &a.body[..]), ("POST", "/frame", "s1", &b"xyz"[..]));
        let b = read_request(&mut r).unwrap().unwrap();
        assert_eq!((b.path.as_str(), b.param("hello"), b.header("connection")), ("/poll", "1", "close"));
        assert!(read_request(&mut r).unwrap().is_none());
    }

    /// A Python client plays the phone: loads the page, polls for the start command and
    /// uploads JPEG frames over one keep-alive connection.
    #[test]
    #[ignore = "opens a LAN port and needs python"]
    fn https_page_and_frames() {
        use std::sync::atomic::AtomicUsize;
        let st = start(None).expect("server");
        let got = Arc::new(AtomicUsize::new(0));
        let g = got.clone();
        set_sink(Some(Box::new(move |jpeg: &[u8]| {
            assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
            g.fetch_add(1, Ordering::SeqCst);
        })));
        send(serde_json::json!({ "cmd": "start", "facing": "back" }));
        let url = st.urls[0].clone();
        let out = std::process::Command::new(if cfg!(windows) { "python" } else { "python3" })
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/iphone_client.py"))
            .arg(&url)
            .output()
            .expect("python");
        println!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success());
        std::thread::sleep(Duration::from_millis(300));
        assert!(got.load(Ordering::SeqCst) >= 10, "frames: {}", got.load(Ordering::SeqCst));
        stop();
    }

    #[test]
    fn has_lan_address() {
        println!("{:?}", local_ips());
    }
}
