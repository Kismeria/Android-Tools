//! Synchronous adb wrapper. Every function blocks; call from worker threads.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::tools;
use crate::util::{err, hidden, Res};

/// Quote for the device shell.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub struct Output {
    pub stdout: Vec<u8>,
    pub text: String,
    pub stderr: String,
    pub ok: bool,
}

pub fn run_raw(serial: Option<&str>, args: &[&str], timeout: Duration) -> Res<Output> {
    tools::ensure()?;
    let mut cmd = hidden(tools::adb_exe());
    if let Some(s) = serial {
        cmd.args(["-s", s]);
    }
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("adb: {e}"))?;

    // Read pipes on threads so a chatty process cannot deadlock, and enforce the timeout.
    let mut out = child.stdout.take().unwrap();
    let mut errp = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut errp, &mut b);
        b
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(err)? {
            break st;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err(format!("Таймаут: adb {}", args.iter().take(2).cloned().collect::<Vec<_>>().join(" ")));
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).trim().to_string();
    let text = String::from_utf8_lossy(&stdout).replace("\r\n", "\n");
    Ok(Output { stdout, text, stderr, ok: status.success() })
}

pub fn run(serial: Option<&str>, args: &[&str], timeout_s: u64) -> Res<String> {
    let o = run_raw(serial, args, Duration::from_secs(timeout_s))?;
    if !o.ok {
        let msg = if o.stderr.is_empty() { o.text.trim().to_string() } else { o.stderr.clone() };
        return Err(msg.lines().last().unwrap_or("adb: ошибка").to_string());
    }
    Ok(o.text)
}

pub fn run_lenient(serial: Option<&str>, args: &[&str], timeout_s: u64) -> Res<String> {
    let o = run_raw(serial, args, Duration::from_secs(timeout_s))?;
    Ok(format!("{}{}", o.text, o.stderr))
}

pub fn shell(serial: &str, command: &str) -> Res<String> {
    run(Some(serial), &["shell", command], 30)
}

pub fn shell_lenient(serial: &str, command: &str) -> Res<String> {
    Ok(run_raw(Some(serial), &["shell", command], Duration::from_secs(60))?.text)
}

fn find<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let i = text.find(key)? + key.len();
    let rest = &text[i..];
    let end = rest.find(|c: char| c == '\n' || c == ' ').unwrap_or(rest.len());
    Some(rest[..end].trim())
}

// ── devices ──

#[derive(Serialize, Clone)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: String,
    pub wireless: bool,
}

pub fn devices() -> Res<Vec<Device>> {
    let out = run(None, &["devices", "-l"], 10)?;
    Ok(out
        .lines()
        .skip(1)
        .filter(|l| !l.starts_with('*'))
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                return None;
            }
            let model = parts
                .iter()
                .find_map(|t| t.strip_prefix("model:"))
                .unwrap_or("")
                .replace('_', " ");
            let serial = parts[0].to_string();
            Some(Device {
                wireless: serial.contains(':') || serial.contains("._adb-tls-connect"),
                serial,
                state: parts[1].to_string(),
                model,
            })
        })
        .collect())
}

pub fn device_info(serial: &str) -> Res<Value> {
    let script = "getprop ro.product.manufacturer; echo '#'; getprop ro.product.model; echo '#';\
        getprop ro.build.version.release; echo '#'; getprop ro.build.version.sdk; echo '#';\
        dumpsys battery | grep -E ' level:| status:'; echo '#';\
        df -k /data | tail -1; echo '#'; wm size; echo '#';\
        ip -f inet addr show wlan0 2>/dev/null | grep inet; echo '#';\
        getprop ro.product.marketname; getprop ro.config.marketing_name";
    let out = shell_lenient(serial, script)?;
    let mut p: Vec<String> = out.split('#').map(|s| s.trim().to_string()).collect();
    p.resize(9, String::new());

    let mut brand = p[0].to_lowercase();
    if let Some(first) = brand.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let battery = find(&p[4], "level:").and_then(|v| v.parse::<u32>().ok());
    let charging = matches!(find(&p[4], "status:"), Some("2") | Some("5"));
    let cols: Vec<&str> = p[5].split_whitespace().collect();
    let (total, used) = if cols.len() >= 4 {
        (cols[1].parse::<u64>().unwrap_or(0) * 1024, cols[2].parse::<u64>().unwrap_or(0) * 1024)
    } else {
        (0, 0)
    };
    let resolution = find(&p[6], "size:").unwrap_or("").replace('x', "×");
    let ip = find(&p[7], "inet ").map(|v| v.split('/').next().unwrap_or("").to_string()).unwrap_or_default();
    let market = p[8].lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let name = if market.is_empty() {
        format!("{brand} {}", p[1])
    } else if market.to_lowercase().contains(&brand.to_lowercase()) {
        market.to_string()
    } else {
        format!("{brand} {market}")
    };
    Ok(json!({
        "name": name.trim(),
        "brand": brand,
        "model": p[1],
        "android": p[2],
        "sdk": p[3].parse::<u32>().unwrap_or(0),
        "battery": battery,
        "charging": charging,
        "storage_total": total,
        "storage_used": used,
        "resolution": resolution,
        "ip": ip,
    }))
}

pub fn connect(address: &str) -> Res<String> {
    let out = run_lenient(None, &["connect", address], 15)?.trim().to_string();
    if !out.contains("connected") || out.contains("cannot") || out.contains("failed") {
        return Err(if out.is_empty() { "Не удалось подключиться".into() } else { out });
    }
    Ok(out)
}

pub fn disconnect(address: &str) -> Res<String> {
    run_lenient(None, &["disconnect", address], 10)
}

pub fn pair(address: &str, code: &str) -> Res<String> {
    let out = run_lenient(None, &["pair", address, code], 20)?;
    if !out.contains("Successfully") {
        return Err(out.lines().last().unwrap_or("Сопряжение не удалось").to_string());
    }
    Ok(out)
}

pub fn to_wifi(serial: &str) -> Res<String> {
    let info = device_info(serial)?;
    let ip = info["ip"].as_str().unwrap_or("").to_string();
    if ip.is_empty() {
        return Err("Телефон не в Wi‑Fi сети".into());
    }
    run(Some(serial), &["tcpip", "5555"], 15)?;
    let address = format!("{ip}:5555");
    let mut last = String::new();
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(800));
        match connect(&address) {
            Ok(_) => return Ok(address),
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn mdns() -> Res<Vec<String>> {
    let out = run_lenient(None, &["mdns", "services"], 10)?;
    Ok(out
        .lines()
        .skip(1)
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 3 && p[1].contains("_adb-tls-connect")).then(|| p[2].to_string())
        })
        .collect())
}

// ── apps ──

fn package_names(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(|s| s.trim().to_string()).collect()
}

pub fn packages(serial: &str, kind: &str) -> Res<Value> {
    let flag = match kind {
        "system" => " -s",
        "all" => "",
        _ => " -3",
    };
    let listed = package_names(&shell(serial, &format!("pm list packages{flag}"))?);
    let disabled: HashSet<String> = package_names(&shell_lenient(serial, "pm list packages -d")?).into_iter().collect();
    let mut items: Vec<Value> = listed
        .into_iter()
        .map(|p| json!({ "package": p, "disabled": disabled.contains(&p) }))
        .collect();
    items.sort_by(|a, b| a["package"].as_str().cmp(&b["package"].as_str()));
    Ok(Value::Array(items))
}

pub fn package_details(serial: &str, package: &str) -> Res<Value> {
    let out = shell_lenient(serial, &format!("dumpsys package {}", q(package)))?;
    let get = |k: &str| {
        out.find(&format!("{k}="))
            .map(|i| out[i + k.len() + 1..].lines().next().unwrap_or("").trim().to_string())
            .map(|v| if k == "targetSdk" { v.split_whitespace().next().unwrap_or("").to_string() } else { v })
    };
    Ok(json!({
        "version": get("versionName"),
        "target_sdk": get("targetSdk"),
        "installed": get("firstInstallTime"),
        "updated": get("lastUpdateTime"),
    }))
}

pub fn install(serial: &str, path: &str) -> Res<String> {
    let p = Path::new(path);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "apk" {
        return run(Some(serial), &["install", "-r", "-d", path], 600);
    }
    // Split bundles (.apks / .xapk / .apkm): unpack the APKs and install them together.
    let tmp = std::env::temp_dir().join(format!("atools_{}", rand::random::<u32>()));
    fs::create_dir_all(&tmp).map_err(err)?;
    let result = (|| {
        let mut zip = zip::ZipArchive::new(fs::File::open(p).map_err(err)?).map_err(err)?;
        let mut files = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(err)?;
            let name = entry.name().to_string();
            if !name.to_lowercase().ends_with(".apk") {
                continue;
            }
            let target = tmp.join(format!("{}_{}", files.len(), Path::new(&name).file_name().unwrap().to_string_lossy()));
            let mut f = fs::File::create(&target).map_err(err)?;
            std::io::copy(&mut entry, &mut f).map_err(err)?;
            files.push(target.display().to_string());
        }
        if files.is_empty() {
            return Err("В архиве нет APK".to_string());
        }
        let mut args = vec!["install-multiple", "-r", "-d"];
        args.extend(files.iter().map(String::as_str));
        run(Some(serial), &args, 900)
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

pub fn uninstall(serial: &str, package: &str) -> Res<()> {
    let out = run_lenient(Some(serial), &["uninstall", package], 60)?;
    if out.contains("Success") {
        return Ok(());
    }
    // System apps can only be removed for the current user.
    let out = shell_lenient(serial, &format!("pm uninstall --user 0 {}", q(package)))?;
    if out.contains("Success") {
        Ok(())
    } else {
        Err(if out.trim().is_empty() { "Не удалось удалить".into() } else { out.trim().to_string() })
    }
}

pub fn launch(serial: &str, package: &str) -> Res<()> {
    let out = shell_lenient(serial, &format!("monkey -p {} -c android.intent.category.LAUNCHER 1", q(package)))?;
    if out.contains("No activities found") {
        return Err("У приложения нет окна запуска".into());
    }
    Ok(())
}

pub fn force_stop(serial: &str, package: &str) -> Res<()> {
    shell(serial, &format!("am force-stop {}", q(package))).map(|_| ())
}

pub fn clear_data(serial: &str, package: &str) -> Res<()> {
    let out = shell_lenient(serial, &format!("pm clear {}", q(package)))?;
    if out.contains("Success") { Ok(()) } else { Err(out.trim().to_string()) }
}

pub fn set_enabled(serial: &str, package: &str, enabled: bool) -> Res<()> {
    let cmd = if enabled { "enable" } else { "disable-user --user 0" };
    let out = shell_lenient(serial, &format!("pm {cmd} {}", q(package)))?;
    if out.contains("new state") { Ok(()) } else { Err(out.trim().to_string()) }
}

pub fn extract_apk(serial: &str, package: &str, dest: &Path) -> Res<PathBuf> {
    let paths = package_names(&shell(serial, &format!("pm path {}", q(package)))?);
    if paths.is_empty() {
        return Err("APK не найден".into());
    }
    fs::create_dir_all(dest).map_err(err)?;
    if paths.len() == 1 {
        let target = dest.join(format!("{package}.apk"));
        run(Some(serial), &["pull", &paths[0], &target.display().to_string()], 600)?;
        return Ok(target);
    }
    let target = dest.join(format!("{package}.apks"));
    let tmp = std::env::temp_dir().join(format!("atools_{}", rand::random::<u32>()));
    fs::create_dir_all(&tmp).map_err(err)?;
    let result = (|| {
        let mut zip = zip::ZipWriter::new(fs::File::create(&target).map_err(err)?);
        for remote in &paths {
            let name = remote.rsplit('/').next().unwrap_or("split.apk").to_string();
            let local = tmp.join(&name);
            run(Some(serial), &["pull", remote, &local.display().to_string()], 600)?;
            zip.start_file(name, zip::write::SimpleFileOptions::default()).map_err(err)?;
            std::io::copy(&mut fs::File::open(&local).map_err(err)?, &mut zip).map_err(err)?;
        }
        zip.finish().map_err(err)?;
        Ok(target.clone())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

// ── files ──

pub fn list_dir(serial: &str, path: &str) -> Res<Value> {
    let dir = format!("{}/", path.trim_end_matches('/'));
    let out = shell_lenient(serial, &format!("cd {} && stat -L -c '%F|%s|%Y|%n' * .* 2>/dev/null", q(&dir)))?;
    let mut items: Vec<Value> = out
        .lines()
        .filter_map(|line| {
            let p: Vec<&str> = line.splitn(4, '|').collect();
            if p.len() != 4 || matches!(p[3], "." | ".." | "*" | ".*") {
                return None;
            }
            Some(json!({
                "name": p[3],
                "dir": p[0] == "directory",
                "size": p[1].parse::<u64>().unwrap_or(0),
                "mtime": p[2].parse::<u64>().unwrap_or(0),
            }))
        })
        .collect();
    items.sort_by(|a, b| {
        let da = a["dir"].as_bool().unwrap_or(false);
        let db = b["dir"].as_bool().unwrap_or(false);
        db.cmp(&da).then_with(|| {
            a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(Value::Array(items))
}

pub fn push(serial: &str, local: &str, remote_dir: &str) -> Res<String> {
    run(Some(serial), &["push", local, &format!("{}/", remote_dir.trim_end_matches('/'))], 3600)
}

pub fn pull(serial: &str, remote: &str, local_dir: &str) -> Res<String> {
    run(Some(serial), &["pull", remote, local_dir], 3600)
}

// ── control ──

pub fn keyevent(serial: &str, key: &str) -> Res<()> {
    let code = match key {
        "power" => 26,
        "home" => 3,
        "back" => 4,
        "recent" => 187,
        "vol_up" => 24,
        "vol_down" => 25,
        "mute" => 164,
        "wake" => 224,
        "play" => 85,
        _ => return Err("Неизвестная кнопка".into()),
    };
    shell(serial, &format!("input keyevent {code}")).map(|_| ())
}

pub fn screenshot(serial: &str) -> Res<Vec<u8>> {
    let o = run_raw(Some(serial), &["exec-out", "screencap", "-p"], Duration::from_secs(20))?;
    if !o.stdout.starts_with(b"\x89PNG") {
        return Err("Скриншот не получен".into());
    }
    Ok(o.stdout)
}

pub fn send_text(serial: &str, text: &str) -> Res<()> {
    if !text.is_ascii() {
        return Err("Только латиница — кириллицу вставляйте через Ctrl+V в окне «Экран»".into());
    }
    let mut escaped = String::new();
    for c in text.chars() {
        match c {
            '\\' | '"' | '$' | '`' => {
                escaped.push('\\');
                escaped.push(c)
            }
            ' ' => escaped.push_str("%s"),
            _ => escaped.push(c),
        }
    }
    shell(serial, &format!("input text \"{escaped}\"")).map(|_| ())
}

pub fn open_url(serial: &str, url: &str) -> Res<()> {
    let url = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
    shell(serial, &format!("am start -a android.intent.action.VIEW -d {}", q(&url))).map(|_| ())
}

pub fn is_locked(serial: &str) -> bool {
    shell_lenient(serial, "dumpsys window | grep -E 'mDreamingLockscreen=|isStatusBarKeyguard=|mShowingLockscreen='")
        .map(|o| {
            o.contains("mDreamingLockscreen=true") || o.contains("isStatusBarKeyguard=true") || o.contains("mShowingLockscreen=true")
        })
        .unwrap_or(false)
}

pub fn open_camera_app(serial: &str, front: bool) -> Res<()> {
    let f = if front { 1 } else { 0 };
    let cmd = format!(
        "am start -a android.media.action.STILL_IMAGE_CAMERA --ei android.intent.extras.CAMERA_FACING {f} \
         --ei android.intent.extras.LENS_FACING_FRONT {f} --ei android.intent.extras.LENS_FACING_BACK {} \
         --ez android.intent.extra.USE_FRONT_CAMERA {}",
        1 - f,
        front
    );
    shell_lenient(serial, &cmd).map(|_| ())
}

pub fn list_cameras(serial: &str) -> Res<Value> {
    let exe = tools::scrcpy_exe().ok_or("scrcpy не найден")?;
    let out = hidden(&exe)
        .args(["-s", serial, "--list-cameras"])
        .env("ADB", tools::adb_exe())
        .current_dir(exe.parent().unwrap())
        .output()
        .map_err(err)?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let cams: Vec<Value> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("--camera-id=")?;
            let id = rest.split_whitespace().next()?;
            let inner = rest.split('(').nth(1)?.trim_end_matches(')');
            let mut parts = inner.split(',').map(str::trim);
            Some(json!({ "id": id, "facing": parts.next()?, "size": parts.next()? }))
        })
        .collect();
    Ok(Value::Array(cams))
}
