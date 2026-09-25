//! Names and icons for the Apps tab.
//! Names: scrcpy `--list-apps` (PackageManager labels, in the phone's language).
//! Icons: read from each APK on the phone with `unzip -p` — manifest → resource table → the
//! best bitmap (or the layers of an adaptive icon) — without pulling whole APKs. Icons are
//! cached on disk by APK path, which changes whenever the app is updated.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apk_info_axml::structs::{ResTableEntry, ResourceValueType};
use apk_info_axml::{ARSC, AXML};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::util::{data_dir, hidden, Res};
use crate::{adb, tools};

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Icon {
    /// CSS background layers, bottom first (bitmaps and SVGs as data URLs, colours as gradients).
    /// Adaptive icons draw 108dp layers of which the middle 72dp are visible: `adaptive` zooms in.
    Layers { layers: Vec<String>, adaptive: bool },
    None,
}

#[derive(Serialize, Clone)]
struct IconEvent<'a> {
    package: &'a str,
    icon: &'a Icon,
}

// ── labels ──

static LABELS: Mutex<Option<(String, HashMap<String, String>)>> = Mutex::new(None);

/// Package → label for every launchable app. Cached per device for the session.
pub fn labels(serial: &str, refresh: bool) -> Res<HashMap<String, String>> {
    if !refresh {
        if let Some((s, map)) = LABELS.lock().unwrap().as_ref() {
            if s == serial {
                return Ok(map.clone());
            }
        }
    }
    tools::ensure()?;
    let exe = tools::scrcpy_exe().ok_or("scrcpy не найден")?;
    let out = hidden(&exe)
        .args(["-s", serial, "--list-apps"])
        .env("ADB", tools::adb_exe())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let map = parse_labels(&text);
    *LABELS.lock().unwrap() = Some((serial.to_string(), map.clone()));
    Ok(map)
}

/// Lines look like ` * Chrome                         com.android.chrome` (`*` system, `-` user).
fn parse_labels(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let l = l.strip_prefix(" * ").or_else(|| l.strip_prefix(" - "))?;
            let package = l.split_whitespace().last()?;
            let label = l[..l.len() - package.len()].trim();
            (!label.is_empty() && package.contains('.')).then(|| (package.to_string(), label.to_string()))
        })
        .collect()
}

// ── icons ──

static GENERATION: AtomicU64 = AtomicU64::new(0);

fn cache_dir() -> PathBuf {
    data_dir().join("app-icons")
}

fn cache_path(apk: &str) -> PathBuf {
    // FNV-1a of the APK path.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in apk.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    cache_dir().join(format!("{h:016x}.json"))
}

/// Loads icons for `packages` in the background, emitting `app-icon` events; a newer call
/// cancels the previous one.
pub fn load(app: AppHandle, serial: String, packages: Vec<String>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let paths = apk_paths(&serial);
        let queue = Arc::new(Mutex::new(packages.into_iter().rev().collect::<Vec<_>>()));
        let _ = std::fs::create_dir_all(cache_dir());
        let workers: Vec<_> = (0..3)
            .map(|_| {
                let (app, serial, paths, queue) = (app.clone(), serial.clone(), paths.clone(), queue.clone());
                std::thread::spawn(move || loop {
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let Some(package) = queue.lock().unwrap().pop() else { return };
                    let Some(apk) = paths.get(&package) else { continue };
                    let cache = cache_path(apk);
                    let icon = std::fs::read(&cache)
                        .ok()
                        .and_then(|d| serde_json::from_slice::<Icon>(&d).ok())
                        .unwrap_or_else(|| {
                            let icon = extract(&serial, apk).unwrap_or(Icon::None);
                            if let Ok(json) = serde_json::to_vec(&icon) {
                                let _ = std::fs::write(&cache, json);
                            }
                            icon
                        });
                    if !matches!(icon, Icon::None) {
                        let _ = app.emit("app-icon", IconEvent { package: &package, icon: &icon });
                    }
                })
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
    });
}

/// `package:/data/app/…/base.apk=com.example` for every package, in one call.
fn apk_paths(serial: &str) -> Arc<HashMap<String, String>> {
    let text = adb::shell_lenient(serial, "pm list packages -f").unwrap_or_default();
    Arc::new(
        text.lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix("package:")?;
                let (path, package) = rest.rsplit_once('=')?;
                Some((package.to_string(), path.to_string()))
            })
            .collect(),
    )
}

fn read_entry(serial: &str, apk: &str, entry: &str) -> Option<Vec<u8>> {
    let cmd = format!("unzip -p {} {}", adb::q(apk), adb::q(entry));
    let out = adb::run_raw(Some(serial), &["exec-out", &cmd], Duration::from_secs(30)).ok()?;
    (out.ok && !out.stdout.is_empty()).then_some(out.stdout)
}

/// `@7f0d0000` → 0x7f0d0000
fn reference(value: &str) -> Option<u32> {
    u32::from_str_radix(value.strip_prefix('@')?, 16).ok()
}

/// Every value of a resource across configurations, following references: (density, value).
fn values(arsc: &ARSC, id: u32, depth: u8, out: &mut Vec<(u16, String)>) {
    if depth > 4 {
        return;
    }
    let (package_id, type_id, entry_id) = (id >> 24, ((id >> 16) & 0xff) as u8, (id & 0xffff) as u16);
    for package in arsc.packages().filter(|p| p.header.id == package_id) {
        for (config, types) in &package.resources {
            let Some(ResTableEntry::Default(e)) = types.get(&type_id).and_then(|t| t.find(entry_id)) else { continue };
            match e.value.data_type {
                ResourceValueType::Reference if e.value.data != id => values(arsc, e.value.data, depth + 1, out),
                _ => out.push((config.get_orientation_touchscreen_density().2, arsc.value_to_string(&e.value))),
            }
        }
    }
}

fn density_rank(d: u16) -> u32 {
    match d {
        0 => 160,
        0xFFFE | 0xFFFF => 1,
        d => d as u32,
    }
}

fn is_bitmap(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    (p.ends_with(".png") || p.ends_with(".webp") || p.ends_with(".jpg")) && !p.ends_with(".9.png")
}

/// The sharpest bitmap among the values (anydpi / nodpi rank below real densities).
fn best_bitmap(candidates: &[(u16, String)]) -> Option<&str> {
    candidates
        .iter()
        .filter(|(_, v)| is_bitmap(v))
        .max_by_key(|(d, _)| density_rank(*d))
        .map(|(_, v)| v.as_str())
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn bitmap_mime(path: &str) -> &'static str {
    if path.ends_with(".webp") { "image/webp" } else if path.ends_with(".jpg") { "image/jpeg" } else { "image/png" }
}

/// `#aarrggbb` / `#rrggbb` / `#argb` / `#rgb` (Android) → CSS.
fn css_color(v: &str) -> Option<String> {
    let hex = v.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        8 => Some(format!("#{}{}", &hex[2..], &hex[..2])),
        4 => Some(format!("#{}{}", &hex[1..], &hex[..1])),
        6 | 3 => Some(format!("#{hex}")),
        _ => None,
    }
}

fn image_layer(url: &str, adaptive: bool) -> String {
    format!("url({url}) center / {} no-repeat", if adaptive { "150% 150%" } else { "contain" })
}

fn color_layer(c: &str) -> String {
    format!("linear-gradient({c}, {c})")
}

/// Resolves drawables of one APK into CSS layers.
struct Resolver<'a> {
    serial: &'a str,
    apk: &'a str,
    arsc: &'a ARSC,
    adaptive: bool,
    svg_ids: u32,
}

impl Resolver<'_> {
    fn read(&self, entry: &str) -> Option<Vec<u8>> {
        read_entry(self.serial, self.apk, entry)
    }

    fn xml(&self, path: &str) -> Option<AXML> {
        let data = self.read(path)?;
        AXML::new(&mut &data[..], None).ok()
    }

    /// `@7f…` reference, `#colour`, or nothing.
    fn value(&mut self, v: &str, depth: u8) -> Option<Vec<String>> {
        if let Some(c) = css_color(v) {
            return Some(vec![color_layer(&c)]);
        }
        let id = reference(v)?;
        let mut found = Vec::new();
        values(self.arsc, id, 0, &mut found);
        if let Some(path) = best_bitmap(&found) {
            let url = data_url(bitmap_mime(path), &self.read(path)?);
            return Some(vec![image_layer(&url, self.adaptive)]);
        }
        if let Some(c) = found.iter().find_map(|(_, v)| css_color(v)) {
            return Some(vec![color_layer(&c)]);
        }
        let mut xmls: Vec<&(u16, String)> = found.iter().filter(|(_, v)| v.ends_with(".xml")).collect();
        xmls.sort_by_key(|(d, _)| std::cmp::Reverse(density_rank(*d)));
        let path = xmls.first()?.1.clone();
        let doc = self.xml(&path)?;
        self.element(&doc.root, depth + 1)
    }

    /// A colour attribute of a vector: literal, @reference to a colour, or a gradient XML.
    fn paint(&mut self, v: Option<&str>, defs: &mut String) -> Option<String> {
        let v = v?;
        if let Some(c) = css_color(v) {
            return Some(c);
        }
        let id = reference(v)?;
        let mut found = Vec::new();
        values(self.arsc, id, 0, &mut found);
        if let Some(c) = found.iter().find_map(|(_, v)| css_color(v)) {
            return Some(c);
        }
        let path = found.iter().map(|(_, v)| v).find(|v| v.ends_with(".xml"))?.clone();
        let doc = self.xml(&path)?;
        let g = &doc.root;
        match g.name() {
            "gradient" => {
                self.svg_ids += 1;
                let gid = format!("g{}", self.svg_ids);
                let mut stops = String::new();
                let items: Vec<_> = g.childrens().filter(|c| c.name() == "item").collect();
                if items.is_empty() {
                    for (off, key) in [(0.0, "startColor"), (0.5, "centerColor"), (1.0, "endColor")] {
                        if let Some(c) = g.attr(key).and_then(css_color) {
                            stops += &format!(r#"<stop offset="{off}" stop-color="{c}"/>"#);
                        }
                    }
                } else {
                    for it in items {
                        let c = it.attr("color").and_then(css_color).unwrap_or_else(|| "#000".into());
                        stops += &format!(r#"<stop offset="{}" stop-color="{c}"/>"#, it.attr("offset").unwrap_or("0"));
                    }
                }
                let a = |k: &str| g.attr(k).unwrap_or("0").to_string();
                if g.attr("type") == Some("1") || g.attr("type") == Some("radial") {
                    *defs += &format!(
                        r#"<radialGradient id="{gid}" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" r="{}">{stops}</radialGradient>"#,
                        a("centerX"), a("centerY"), a("gradientRadius")
                    );
                } else {
                    *defs += &format!(
                        r#"<linearGradient id="{gid}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">{stops}</linearGradient>"#,
                        a("startX"), a("startY"), a("endX"), a("endY")
                    );
                }
                Some(format!("url(#{gid})"))
            }
            "selector" => g.childrens().find_map(|c| c.attr("color")).and_then(css_color),
            _ => None,
        }
    }

    /// Android VectorDrawable → SVG markup for the children of `el`.
    fn vector_body(&mut self, el: &apk_info_xml::Element, defs: &mut String) -> String {
        let esc = |s: &str| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let mut out = String::new();
        let mut open_clips = 0;
        for c in el.childrens() {
            match c.name() {
                "path" => {
                    let Some(d) = c.attr("pathData") else { continue };
                    let fill = self.paint(c.attr("fillColor"), defs).unwrap_or_else(|| "none".into());
                    let stroke = self.paint(c.attr("strokeColor"), defs).unwrap_or_else(|| "none".into());
                    let rule = if c.attr("fillType") == Some("1") || c.attr("fillType") == Some("evenOdd") { "evenodd" } else { "nonzero" };
                    out += &format!(
                        r#"<path d="{}" fill="{fill}" fill-opacity="{}" fill-rule="{rule}" stroke="{stroke}" stroke-width="{}" stroke-opacity="{}"/>"#,
                        esc(d),
                        c.attr("fillAlpha").unwrap_or("1"),
                        c.attr("strokeWidth").unwrap_or("0"),
                        c.attr("strokeAlpha").unwrap_or("1"),
                    );
                }
                "clip-path" => {
                    let Some(d) = c.attr("pathData") else { continue };
                    self.svg_ids += 1;
                    *defs += &format!(r#"<clipPath id="c{}"><path d="{}"/></clipPath>"#, self.svg_ids, esc(d));
                    out += &format!(r#"<g clip-path="url(#c{})">"#, self.svg_ids);
                    open_clips += 1;
                }
                "group" => {
                    let n = |k: &str, def: f32| c.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(def);
                    let (px, py) = (n("pivotX", 0.0), n("pivotY", 0.0));
                    let transform = format!(
                        "translate({} {}) rotate({}) scale({} {}) translate({} {})",
                        n("translateX", 0.0) + px, n("translateY", 0.0) + py, n("rotation", 0.0),
                        n("scaleX", 1.0), n("scaleY", 1.0), -px, -py
                    );
                    let inner = self.vector_body(c, defs);
                    out += &format!(r#"<g transform="{transform}">{inner}</g>"#);
                }
                _ => {}
            }
        }
        out += &"</g>".repeat(open_clips);
        out
    }

    fn vector(&mut self, el: &apk_info_xml::Element) -> Option<String> {
        let dim = |k: &str| el.attr(k).map(|v| v.trim_end_matches(|c: char| c.is_ascii_alphabetic()).to_string());
        let (vw, vh) = (dim("viewportWidth")?, dim("viewportHeight")?);
        let mut defs = String::new();
        let body = self.vector_body(el, &mut defs);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {vw} {vh}" width="{vw}" height="{vh}"><defs>{defs}</defs>{body}</svg>"#
        );
        Some(data_url("image/svg+xml", svg.as_bytes()))
    }

    /// Any drawable element → layers (bottom first).
    fn element(&mut self, el: &apk_info_xml::Element, depth: u8) -> Option<Vec<String>> {
        if depth > 6 {
            return None;
        }
        let via = |this: &mut Self, attr: &str| -> Option<Vec<String>> {
            if let Some(v) = el.attr(attr) {
                if let Some(l) = this.value(v, depth) {
                    return Some(l);
                }
            }
            el.childrens().find_map(|c| this.element(c, depth + 1))
        };
        match el.name() {
            "adaptive-icon" => {
                self.adaptive = true;
                let mut layers = Vec::new();
                for tag in ["background", "foreground"] {
                    if let Some(c) = el.childrens().find(|c| c.name() == tag) {
                        layers.extend(self.element(c, depth + 1).unwrap_or_default());
                    }
                }
                (!layers.is_empty()).then_some(layers)
            }
            "background" | "foreground" | "item" | "inset" | "scale" | "clip" | "rotate" | "animated-rotate"
            | "animated-vector" => via(self, "drawable"),
            "bitmap" | "nine-patch" => via(self, "src"),
            "layer-list" => {
                let layers: Vec<String> = el.childrens().filter_map(|c| self.element(c, depth + 1)).flatten().collect();
                (!layers.is_empty()).then_some(layers)
            }
            "selector" | "level-list" | "transition" | "ripple" | "animation-list" => {
                el.childrens().find_map(|c| self.element(c, depth + 1))
            }
            "vector" => {
                let url = self.vector(el)?;
                Some(vec![image_layer(&url, self.adaptive)])
            }
            "shape" => {
                let solid = el.childrens().find(|c| c.name() == "solid").and_then(|s| s.attr("color"));
                let grad = el.childrens().find(|c| c.name() == "gradient").and_then(|g| g.attr("startColor"));
                let c = solid.or(grad).and_then(css_color)?;
                Some(vec![color_layer(&c)])
            }
            "color" => el.attr("color").and_then(css_color).map(|c| vec![color_layer(&c)]),
            _ => None,
        }
    }
}

fn extract(serial: &str, apk: &str) -> Option<Icon> {
    let manifest = read_entry(serial, apk, "AndroidManifest.xml")?;
    let axml = AXML::new(&mut &manifest[..], None).ok()?;
    let icon = axml.get_attribute_value("application", "icon", None)?;
    let table = read_entry(serial, apk, "resources.arsc")?;
    let arsc = ARSC::new(&mut &table[..]).ok()?;
    let mut r = Resolver { serial, apk, arsc: &arsc, adaptive: false, svg_ids: 0 };
    let layers = r.value(&icon, 0)?;
    Some(Icon::Layers { layers, adaptive: r.adaptive })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrcpy_app_list() {
        let text = "[server] INFO: List of apps:\n * Chrome                         com.android.chrome\n - Google Play Музыка             com.google.android.music\n - Telegram org.telegram.messenger\nINFO: done";
        let m = parse_labels(text);
        assert_eq!(m["com.android.chrome"], "Chrome");
        assert_eq!(m["com.google.android.music"], "Google Play Музыка");
        assert_eq!(m["org.telegram.messenger"], "Telegram");
        assert_eq!(m.len(), 3);
    }

    /// ATOOLS_SERIAL=<device> ATOOLS_GALLERY=<file.html> cargo test real_phone_icons -- --ignored --nocapture
    #[test]
    #[ignore = "needs a phone"]
    fn real_phone_icons() {
        let serial = std::env::var("ATOOLS_SERIAL").expect("ATOOLS_SERIAL");
        let paths = apk_paths(&serial);
        let user = adb::shell_lenient(&serial, "pm list packages -3").unwrap_or_default();
        let mut packages: Vec<&str> = user.lines().filter_map(|l| l.trim().strip_prefix("package:")).collect();
        packages.sort();
        let names = labels(&serial, true).unwrap_or_default();
        let started = std::time::Instant::now();
        let mut html = String::from("<body style='background:#222;color:#eee;font:12px sans-serif;display:grid;grid-template-columns:repeat(6,1fr);gap:10px'>");
        let mut none = Vec::new();
        for package in &packages {
            let icon = extract(&serial, &paths[*package]);
            let style = match &icon {
                Some(Icon::Layers { layers, .. }) => layers.iter().rev().cloned().collect::<Vec<_>>().join(", "),
                _ => { none.push(*package); "#555".into() }
            };
            html += &format!("<div><div style='width:56px;height:56px;border-radius:14px;background:{style}'></div>{}<br><small>{package}</small></div>",
                names.get(*package).map(String::as_str).unwrap_or("?"));
        }
        println!("{} user apps in {:?}, no icon: {:?}", packages.len(), started.elapsed(), none);
        if let Ok(f) = std::env::var("ATOOLS_GALLERY") {
            std::fs::write(f, html).unwrap();
        }
    }

    #[test]
    fn colors_and_references() {
        assert_eq!(css_color("#ff3ddc84").as_deref(), Some("#3ddc84ff"));
        assert_eq!(css_color("#f0a0").as_deref(), Some("#0a0f"));
        assert_eq!(reference("@7f0d0001"), Some(0x7f0d0001));
        assert!(reference("@android:drawable/sym_def_app_icon").is_none());
        let c = vec![(0xFFFE, "res/a.xml".to_string()), (480, "res/b.png".into()), (640, "res/c.webp".into()), (160, "res/d.png".into())];
        assert_eq!(best_bitmap(&c), Some("res/c.webp"));
    }
}
