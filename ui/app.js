"use strict";
/* Android Tools — UI logic. Backend: Tauri commands in src-tauri/src/commands.rs */

const T = window.__TAURI__;
const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => [...root.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/* ───────────── config ───────────── */

const DEFAULTS = {
  lang: "",
  serial: "",
  save_dir: "",
  auto_refresh: true,
  wifi_last: "",
  screen: {
    preset: "balance", max_size: 1600, fps: 60, bitrate: 8, codec: "h264", audio: true, turn_off: false,
    stay_awake: true, touches: false, on_top: false, borderless: false, fullscreen: false, view_only: false,
    uhid: false, uhid_mouse: false, record: false,
  },
  camera: {
    facing: "front", camera_id: "", quality: 720, fps: 30, bitrate: 6, rotation: 0, mirror: false, fill: false,
    preview: true, torch: false, mode: "auto",
  },
  apps: { filter: "user" },
  files: { path: "/sdcard/" },
  utils: { clipboard: true },
};

const PRESETS = {
  fast: { max_size: 1024, fps: 60, bitrate: 4, codec: "h264" },
  balance: { max_size: 1600, fps: 60, bitrate: 8, codec: "h264" },
  max: { max_size: 0, fps: 120, bitrate: 24, codec: "h265" },
};

const cfg = (() => {
  let saved = {};
  try { saved = JSON.parse(localStorage.getItem("cfg") || "{}"); } catch {}
  const merge = (base, over) => {
    const out = structuredClone(base);
    for (const k of Object.keys(over || {})) {
      if (!(k in out)) continue;
      out[k] = out[k] && typeof out[k] === "object" && !Array.isArray(out[k]) ? merge(out[k], over[k]) : over[k];
    }
    return out;
  };
  return merge(DEFAULTS, saved);
})();
const save = () => { try { localStorage.setItem("cfg", JSON.stringify(cfg)); } catch {} };
const getPath = (path) => path.split(".").reduce((o, k) => o[k], cfg);
const setPath = (path, v) => {
  const keys = path.split(".");
  const last = keys.pop();
  keys.reduce((o, k) => o[k], cfg)[last] = v;
  save();
};

/* ───────────── backend, busy, toasts ───────────── */

let busyCount = 0;
const setBusy = (d) => { busyCount = Math.max(0, busyCount + d); $("#busy").classList.toggle("on", busyCount > 0); };

async function call(cmd, args = {}, { busy = true, quiet = false } = {}) {
  if (busy) setBusy(1);
  try {
    return await T.core.invoke(cmd, args);
  } catch (e) {
    if (!quiet) toast(String(e), "err");
    throw e;
  } finally {
    if (busy) setBusy(-1);
  }
}

function toast(text, kind = "info") {
  const icon = { ok: "&#xE73E;", err: "&#xE783;" }[kind] || "&#xE946;";
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.innerHTML = `<i class="ic">${icon}</i><span>${esc(text)}</span>`;
  const box = $("#toasts");
  box.append(el);
  while (box.children.length > 4) box.firstChild.remove();
  setTimeout(() => { el.classList.add("out"); setTimeout(() => el.remove(), 220); }, kind === "err" ? 5200 : 3200);
}

/* ───────────── modal & menu ───────────── */

function modal({ title, text = "", ok = "OK", kind = "primary", input = null }) {
  return new Promise((resolve) => {
    const back = $("#modalBack");
    $("#modalTitle").textContent = title;
    $("#modalText").textContent = text;
    const body = $("#modalBody");
    body.innerHTML = "";
    let field = null;
    if (input !== null) {
      field = document.createElement("input");
      field.type = "text";
      field.value = input.value || "";
      field.placeholder = input.placeholder || "";
      body.append(field);
    }
    const okBtn = $("#modalOk");
    okBtn.textContent = ok;
    okBtn.className = `btn ${kind}`;
    back.classList.add("show");
    const done = (value) => {
      back.classList.remove("show");
      okBtn.onclick = $("#modalCancel").onclick = back.onkeydown = null;
      resolve(value);
    };
    okBtn.onclick = () => done(field ? field.value.trim() || null : true);
    $("#modalCancel").onclick = () => done(field ? null : false);
    back.onkeydown = (e) => {
      if (e.key === "Escape") done(field ? null : false);
      if (e.key === "Enter") okBtn.click();
    };
    setTimeout(() => (field ? (field.focus(), field.select()) : okBtn.focus()), 30);
  });
}
const confirmBox = (title, text, ok = "Да", kind = "danger") => modal({ title, text, ok, kind });
const prompt = (title, value = "", placeholder = "") => modal({ title, ok: "Готово", input: { value, placeholder } });

function menu(x, y, items) {
  const m = $("#menu");
  m.innerHTML = "";
  for (const it of items) {
    if (it === "-") { m.append(document.createElement("hr")); continue; }
    const b = document.createElement("button");
    b.innerHTML = esc(it.label);
    if (it.checked) b.classList.add("checked");
    if (it.danger) b.classList.add("danger");
    b.disabled = !!it.disabled;
    b.onclick = () => { hideMenu(); it.action?.(); };
    m.append(b);
  }
  m.classList.add("show");
  const r = m.getBoundingClientRect();
  m.style.left = Math.min(x, innerWidth - r.width - 8) + "px";
  m.style.top = Math.min(y, innerHeight - r.height - 8) + "px";
}
const hideMenu = () => $("#menu").classList.remove("show");
document.addEventListener("mousedown", (e) => { if (!e.target.closest("#menu")) hideMenu(); });
document.addEventListener("contextmenu", (e) => { if (!e.target.closest("input, textarea, .console")) e.preventDefault(); });

/* ───────────── generic bindings ───────────── */

const segHandlers = new Map();
function bindSeg(seg, onChange) {
  const path = seg.dataset.bind;
  const isInt = seg.dataset.type === "int";
  const paint = () => {
    const v = String(path ? getPath(path) : seg.dataset.value);
    for (const b of seg.querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === v);
  };
  seg.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    const v = isInt ? parseInt(b.dataset.v, 10) : b.dataset.v;
    if (path) setPath(path, v); else seg.dataset.value = v;
    paint();
    onChange?.(v, path);
  });
  seg.paint = paint;
  paint();
}

function bindAll(root, onChange) {
  for (const seg of $$(".seg[data-bind]", root)) bindSeg(seg, onChange);
  for (const cb of $$("input[type=checkbox][data-bind]", root)) {
    cb.checked = !!getPath(cb.dataset.bind);
    cb.addEventListener("change", () => { setPath(cb.dataset.bind, cb.checked); onChange?.(cb.checked, cb.dataset.bind); });
  }
  for (const r of $$("input[type=range][data-bind]", root)) {
    const out = $(`[data-out="${r.dataset.bind}"]`, root);
    const paint = () => {
      const p = ((r.value - r.min) / (r.max - r.min)) * 100;
      r.style.setProperty("--p", p + "%");
      if (out) out.textContent = r.value + (r.dataset.unit || "");
    };
    r.value = getPath(r.dataset.bind);
    paint();
    r.addEventListener("input", paint);
    r.addEventListener("change", () => { setPath(r.dataset.bind, parseInt(r.value, 10)); onChange?.(parseInt(r.value, 10), r.dataset.bind); });
    r.repaint = () => { r.value = getPath(r.dataset.bind); paint(); };
  }
}
const repaint = (root) => {
  for (const s of $$(".seg[data-bind]", root)) s.paint?.();
  for (const c of $$("input[type=checkbox][data-bind]", root)) c.checked = !!getPath(c.dataset.bind);
  for (const r of $$("input[type=range][data-bind]", root)) r.repaint?.();
};

const plural = (n, one, few, many) => {
  const t = n % 100, u = n % 10;
  return `${n} ${t >= 11 && t <= 14 ? many : u === 1 ? one : u >= 2 && u <= 4 ? few : many}`;
};
const humanSize = (n) => {
  const u = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return i ? `${n.toFixed(1)} ${u[i]}` : `${n} ${u[i]}`;
};
const shortPath = (p) => p.replace(/^C:\\Users\\[^\\]+/i, "~").replace(/^\/home\/[^/]+/, "~");
const IS_WINDOWS = navigator.userAgent.includes("Windows");
const joinPath = (...parts) => parts.join(IS_WINDOWS ? "\\" : "/");
// Folder inside the save directory, named in the UI language.
const saveSub = (name) => joinPath(cfg.save_dir, I18N.t(name));

/* ───────────── device state ───────────── */

const S = { devices: [], info: {}, serial: cfg.serial, polling: false, signature: null };
const listeners = new Set();
const onDevice = (fn) => listeners.add(fn);
const emitDevice = () => listeners.forEach((fn) => fn());

const device = () => S.devices.find((d) => d.serial === S.serial);
const online = () => device()?.state === "device";
const devName = (serial = S.serial) => S.info[serial]?.name || S.devices.find((d) => d.serial === serial)?.model || serial;

function requireDevice() {
  const d = device();
  if (!d) { toast("Нет устройства", "err"); return null; }
  if (d.state === "unauthorized") { toast("Разрешите отладку на экране телефона", "err"); return null; }
  if (d.state !== "device") { toast("Устройство не в сети", "err"); return null; }
  return d.serial;
}

function select(serial) {
  if (serial === S.serial) return;
  S.serial = serial;
  cfg.serial = serial;
  save();
  emitDevice();
}

async function poll(force = false) {
  if (S.polling || (!force && !cfg.auto_refresh && S.signature !== null)) return;
  S.polling = true;
  try {
    const list = await call("devices", {}, { busy: force, quiet: !force });
    const sig = list.map((d) => d.serial + d.state).join("|");
    if (sig === S.signature && !force) return;
    S.signature = sig;
    S.devices = list;
    for (const d of list) if (d.state === "device" && !S.info[d.serial]) refreshInfo(d.serial);
    if (!list.some((d) => d.serial === S.serial)) {
      const on = list.filter((d) => d.state === "device");
      S.serial = (list.find((d) => d.serial === cfg.serial) || on[0] || list[0])?.serial || "";
    }
    emitDevice();
  } catch {
    S.devices = [];
    S.signature = null;
    emitDevice();
  } finally {
    S.polling = false;
  }
}

async function refreshInfo(serial) {
  try {
    S.info[serial] = await call("device_info", { serial }, { busy: false, quiet: true });
    emitDevice();
  } catch {}
}

/* ───────────── navigation ───────────── */

const pages = {};
let current = null;

function go(name) {
  if (!pages[name]) return;
  current = name;
  for (const n of $$(".nav")) n.classList.toggle("active", n.dataset.page === name);
  for (const p of $$(".page")) p.classList.toggle("active", p.id === `page-${name}`);
  const page = $(`#page-${name}`);
  $("#title").textContent = page.dataset.title;
  const actions = $("#actions");
  actions.innerHTML = "";
  if (pages[name].actions) actions.append(pages[name].actions);
  pages[name].show?.();
}

function registerPage(name, impl) {
  const el = $(`#page-${name}`);
  const tpl = $("template[data-actions]", el);
  if (tpl) {
    impl.actions = tpl.content.cloneNode(true);
    const holder = document.createElement("div");
    holder.className = "actions";
    holder.append(impl.actions);
    impl.actions = holder;
    $$("[data-act=refresh]", holder).forEach((b) => (b.onclick = () => impl.refresh?.()));
  }
  pages[name] = impl;
  impl.init?.(el, impl.actions);
}

/* ═════════════ DEVICES ═════════════ */

registerPage("devices", {
  init(el) {
    const addr = $("#wifiAddr");
    addr.value = cfg.wifi_last;
    const connect = async () => {
      let a = addr.value.trim();
      if (!a) return;
      if (!a.includes(":")) a += ":5555";
      cfg.wifi_last = a; save();
      await call("wifi_connect", { address: a });
      toast(`Подключено: ${a}`, "ok");
      cfg.serial = a; S.serial = a;
      this.refresh();
    };
    addr.onkeydown = (e) => e.key === "Enter" && connect();
    $("#wifiConnect").onclick = connect;
    const pair = async () => {
      const a = $("#pairAddr").value.trim(), code = $("#pairCode").value.trim();
      if (!a || !code) return toast("Укажите адрес и код", "err");
      await call("wifi_pair", { address: a, code });
      toast("Сопряжено — теперь «Подключить»", "ok");
      if (!addr.value) addr.value = a.split(":")[0] + ":";
      addr.focus();
    };
    $("#pairCode").onkeydown = (e) => e.key === "Enter" && pair();
    $("#wifiPair").onclick = pair;
    $("#wifiScan").onclick = async () => {
      const found = await call("wifi_scan");
      if (!found.length) return toast("В сети ничего не найдено");
      addr.value = found[0];
      toast(`Найдено: ${found.join(", ")}`, "ok");
    };
    onDevice(() => this.render());
    this.render();
  },

  refresh() {
    S.signature = null;
    poll(true);
    for (const s of Object.keys(S.info)) refreshInfo(s);
  },

  render() {
    const list = $("#deviceList");
    $("#deviceEmpty").style.display = S.devices.length ? "none" : "";
    list.innerHTML = "";
    const states = { device: ["В сети", "ok"], unauthorized: ["Нет доступа", "warn"], offline: ["Офлайн", "err"] };
    for (const d of S.devices) {
      const info = S.info[d.serial] || {};
      const [stText, stCls] = states[d.state] || [d.state, "hint"];
      const sel = d.serial === S.serial;
      const sub = d.state === "unauthorized"
        ? "Подтвердите отладку на экране телефона"
        : [info.android && `Android ${info.android}`, info.resolution, d.serial].filter(Boolean).join("  ·  ");
      const ring = (value, color) => {
        const c = 2 * Math.PI * 16;
        return `<svg viewBox="0 0 38 38"><circle class="track" cx="19" cy="19" r="16"/>
          <circle class="bar" cx="19" cy="19" r="16" stroke-dasharray="${c}" stroke-dashoffset="${c * (1 - value)}" style="stroke:${color}"/></svg>`;
      };
      let meters = "";
      if (info.battery != null) {
        meters += `<div class="meter">${ring(info.battery / 100, info.battery > 20 ? "var(--accent)" : "var(--danger)")}
          <div class="hint">${info.battery}%${info.charging ? " ⚡" : ""}</div></div>`;
      }
      if (info.storage_total) {
        const used = info.storage_used / info.storage_total;
        meters += `<div class="meter">${ring(used, used < 0.9 ? "var(--text)" : "var(--danger)")}
          <div class="hint">${humanSize(info.storage_total - info.storage_used)} св.</div></div>`;
      }
      const card = document.createElement("div");
      card.className = `card device${sel ? " selected" : ""}`;
      card.innerHTML = `
        <div class="badge"><i class="ic">${d.wireless ? "&#xE701;" : "&#xE88E;"}</i></div>
        <div class="grow">
          <div class="row"><span class="big">${esc(info.name || d.model || d.serial)}</span>
            <span class="state ${stCls}">●&nbsp; ${stText}</span></div>
          <div class="hint">${esc(sub)}</div>
        </div>
        <div class="row" style="gap:18px">${meters}</div>
        <button class="btn ${sel ? "primary" : ""}" data-use ${d.state !== "device" ? "disabled" : ""}>${sel ? "Активно" : "Выбрать"}</button>
        <button class="btn ghost icon" data-more title="Ещё"><i class="ic">&#xE712;</i></button>`;
      card.onclick = (e) => {
        if (e.target.closest("[data-more]")) {
          const r = e.target.closest("[data-more]").getBoundingClientRect();
          const items = [];
          if (d.state === "device" && !d.wireless) items.push({ label: "Перейти на Wi‑Fi", action: () => this.toWifi(d.serial) });
          if (d.wireless) items.push({ label: "Отключить", action: async () => { await call("wifi_disconnect", { address: d.serial }); this.refresh(); } });
          items.push({ label: "Обновить инфо", action: () => refreshInfo(d.serial) });
          items.push({ label: "Копировать серийник", action: () => navigator.clipboard.writeText(d.serial) });
          if (info.ip) items.push({ label: `Копировать IP (${info.ip})`, action: () => navigator.clipboard.writeText(info.ip) });
          return menu(r.left, r.bottom + 4, items);
        }
        if (d.state === "device") select(d.serial);
      };
      list.append(card);
    }
  },

  async toWifi(serial) {
    toast("Перевод на Wi‑Fi…");
    const a = await call("wifi_switch", { serial });
    $("#wifiAddr").value = a;
    cfg.wifi_last = a; save();
    toast(`Готово: ${a} — USB можно отключить`, "ok");
    this.refresh();
  },
});

/* ═════════════ SCREEN ═════════════ */

registerPage("screen", {
  running: false,
  init(el) {
    bindSeg($("#scrPreset"), (v) => {
      Object.assign(cfg.screen, PRESETS[v]);
      cfg.screen.preset = v; save();
      repaint(el);
      this.paintPreset();
    });
    bindAll(el, (v, path) => {
      if (["screen.max_size", "screen.fps", "screen.bitrate", "screen.codec"].includes(path)) {
        cfg.screen.preset = Object.keys(PRESETS).find((n) => Object.entries(PRESETS[n]).every(([k, x]) => cfg.screen[k] === x)) || "";
        save();
      }
      this.paintPreset();
      this.syncControl();
      if (this.running) $("#scrStatus").textContent = "Перезапустите, чтобы применить";
    });
    this.paintPreset();
    this.syncControl();
    $("#scrStart").onclick = () => this.toggle();
    onDevice(() => ($("#scrDevice").textContent = device() ? devName() : "Нет устройства"));
    setInterval(() => this.watch(), 1000);
  },
  paintPreset() {
    for (const b of $$("#scrPreset button")) b.classList.toggle("on", b.dataset.v === cfg.screen.preset);
  },
  syncControl() {
    for (const s of $$("#page-screen .switch.control")) s.classList.toggle("disabled", cfg.screen.view_only);
  },
  async toggle() {
    if (this.running) {
      await call("mirror_stop", {}, { busy: false });
      return this.setRunning(false);
    }
    const serial = requireDevice();
    if (!serial) return;
    $("#scrStart").disabled = true;
    try {
      this.warned = false;
      await call("mirror_start", {
        serial, title: `${devName()} — Android Tools`, saveDir: cfg.save_dir, settings: cfg.screen,
      });
      this.setRunning(true);
      if (cfg.screen.record) toast("Идёт запись в папку «Записи»", "ok");
    } finally {
      $("#scrStart").disabled = false;
    }
  },
  async watch() {
    if (!this.running) return;
    const st = await call("mirror_state", {}, { busy: false, quiet: true }).catch(() => null);
    if (!st?.running) return this.setRunning(false);
    if (st.input_blocked && !this.warned) {
      this.warned = true;
      this.inputBlocked();
    }
  },
  // Firmware blocks adb input: show the exact setting for this brand and offer HID mouse.
  async inputBlocked() {
    const brand = (S.info[S.serial]?.brand || "").toLowerCase();
    let where = "";
    if (/xiaomi|redmi|poco/.test(brand)) {
      where = "Настройки → Для разработчиков → включите «Отладка по USB (настройки безопасности)». Нужны SIM-карта и вход в Mi-аккаунт.";
    } else if (/oppo|realme|oneplus/.test(brand)) {
      where = "Настройки → Для разработчиков → включите «Отключить мониторинг разрешений».";
    } else if (/vivo|iqoo/.test(brand)) {
      where = "Настройки → Для разработчиков → включите «Отладка по USB (настройки безопасности)», если есть.";
    }
    const text = "Прошивка телефона запрещает управление через adb — поэтому мышь и клавиатура не работают. "
      + (where ? `Исправить: ${where} Затем перезапустите трансляцию. ` : "")
      + "Или включите HID‑мышь и HID‑клавиатуру — они работают без этой настройки.";
    const ok = await modal({ title: "Управление заблокировано", text, ok: "Включить HID и перезапустить", kind: "primary" });
    if (!ok) return;
    cfg.screen.uhid_mouse = true;
    cfg.screen.uhid = true;
    save();
    repaint($("#page-screen"));
    await call("mirror_stop", {}, { busy: false });
    this.setRunning(false);
    this.toggle();
  },
  setRunning(on) {
    this.running = on;
    const b = $("#scrStart");
    b.className = `btn big block ${on ? "stop" : "primary"}`;
    b.innerHTML = on ? `<i class="ic">&#xE71A;</i><span>Остановить</span>` : `<i class="ic">&#xE768;</i><span>Запустить</span>`;
    const st = $("#scrStatus");
    st.textContent = on ? "● Идёт трансляция" : "Остановлено";
    st.className = on ? "ok" : "hint";
    st.style.fontSize = "12px";
  },
});

/* ═════════════ CAMERA ═════════════ */

registerPage("camera", {
  running: false,
  camerasFor: null,
  init(el) {
    const restartKeys = ["camera.facing", "camera.quality", "camera.fps", "camera.bitrate", "camera.torch", "camera.mode"];
    let timer = null;
    bindAll(el, (v, path) => {
      if (path === "camera.facing") { cfg.camera.camera_id = ""; save(); $("#camModule").value = ""; }
      if (path === "camera.mode") this.paintMode();
      if (path === "camera.preview" && !v) this.clearPreview("Превью скрыто");
      if (!this.running) return;
      // Linux streams through scrcpy directly: any change needs a restart.
      if (restartKeys.includes(path) || !IS_WINDOWS) {
        clearTimeout(timer);
        timer = setTimeout(() => this.start(), 700);
      } else {
        call("camera_live", { live: this.live() }, { busy: false, quiet: true });
      }
    });
    $("#camModule").onchange = (e) => {
      cfg.camera.camera_id = e.target.value; save();
      if (this.running) this.start();
    };
    $("#camStart").onclick = () => (this.running ? this.stop() : this.start());
    $("#vcamInstall").onclick = () => this.install();
    $("#vcamRemove").onclick = () => this.remove();

    T.event.listen("camera-frame", (e) => {
      if (!this.running || !cfg.camera.preview) return;
      const box = $("#camPreview");
      $("img", box).src = e.payload;
      box.classList.add("live");
    });
    T.event.listen("camera-status", (e) => {
      const s = e.payload;
      if (s.state === "stopped") return this.setRunning(false);
      $("#camStatus").textContent = s.state === "running"
        ? (this.vcam?.device ? "Трансляция → Android Tools Camera" : "Только превью — камера Windows не установлена")
        : s.text;
      $("#camDot").classList.toggle("on", s.state === "running");
      $("#camFps").textContent = s.state === "running" ? `${Math.round(s.fps)} fps · ${s.native ? "прямой" : "совм."}` : "";
    });
    T.event.listen("camera-error", (e) => toast(e.payload, "err"));

    onDevice(() => this.paintMode());
    this.paintMode();
    this.checkVcam();
    this.clearPreview();
  },
  show() { this.checkVcam(); },
  live() {
    const c = cfg.camera;
    return { rotation: c.rotation, mirror: c.mirror, fill: c.fill, preview: c.preview };
  },
  paintMode() {
    const sdk = S.info[S.serial]?.sdk || 0;
    const native = cfg.camera.mode === "native" || (cfg.camera.mode === "auto" && sdk >= 31);
    const hint = $("#camModeHint");
    if (!device()) hint.textContent = "";
    else if (sdk && sdk < 31 && cfg.camera.mode !== "native")
      hint.textContent = `Android ${S.info[S.serial].android}: прямой доступ к камере — только с Android 12. Используется приложение камеры на телефоне, телефон должен быть разблокирован.`;
    else if (sdk && sdk < 31) hint.textContent = "На этом телефоне прямой режим не сработает — нужен Android 12+.";
    else if (native) hint.textContent = "Прямой доступ к камере — экран телефона можно выключить.";
    else hint.textContent = "Через приложение камеры на телефоне.";
    $("#camTorch").classList.toggle("disabled", !native);
    if (native && online() && this.camerasFor !== S.serial && sdk >= 31) {
      this.camerasFor = S.serial;
      call("camera_list", { serial: S.serial }, { busy: false, quiet: true }).then((cams) => {
        const sel = $("#camModule");
        const names = { front: "фронт.", back: "основн.", external: "внешн." };
        sel.innerHTML = `<option value="">Авто</option>` + cams.map((c) =>
          `<option value="${esc(c.id)}">#${esc(c.id)} · ${names[c.facing] || esc(c.facing)} · ${esc(c.size)}</option>`).join("");
        sel.value = cfg.camera.camera_id;
        $("#camModuleRow").style.display = cams.length > 2 ? "" : "none";
      }).catch(() => {});
    }
    if (!native) $("#camModuleRow").style.display = "none";
  },
  async checkVcam() {
    const st = await call("camera_status", {}, { busy: false, quiet: true }).catch(() => null);
    const label = $("#vcamState");
    if (!st) return;
    this.vcam = st;
    const dshow = st.kind === "dshow";
    const v4l2 = st.kind === "v4l2";
    $("#vcamSection").textContent = v4l2 ? "Системная камера" : "Камера Windows";
    $("#vcamNote").textContent = v4l2
      ? "Linux: камера видна во всех программах (v4l2loopback). Установка один раз, нужен пароль администратора."
      : dshow
      ? "Windows 10: камера видна в Zoom, Discord, Teams, Telegram, OBS, Chrome и Edge. Встроенное приложение «Камера» Windows её не показывает. Установка один раз, нужны права администратора."
      : "Системное устройство: «Камера» Windows, Zoom, Teams, Discord, Telegram, OBS, браузеры. Установка один раз, нужны права администратора.";
    if (v4l2 && !st.supported && !st.device) {
      label.innerHTML = `<span class="warn">Нужен пакет v4l2loopback-dkms</span>`;
      $("#vcamInstall").style.display = "none";
      $("#vcamRemove").style.display = "none";
      return;
    }
    if (st.device) {
      label.innerHTML = `<span class="ok">✓ Установлена</span>${dshow ? " · DirectShow" : ""}`;
    } else if (st.registered) {
      label.innerHTML = `<span class="warn">Зарегистрирована, но не видна${st.service_disabled ? " — служба камер отключена" : ""}</span>`;
    } else {
      label.innerHTML = `<span class="warn">Не установлена</span>${st.service_disabled ? " · служба камер Windows отключена, установка включит её" : ""}`;
    }
    $("#vcamInstall").style.display = st.device ? "none" : "";
    $("#vcamRemove").style.display = st.registered ? "" : "none";
  },
  async install() {
    const ok = await confirmBox("Установить камеру?",
      this.vcam?.kind === "v4l2"
        ? "Linux: камера видна во всех программах (v4l2loopback). Установка один раз, нужен пароль администратора."
        : "Windows запросит права администратора. Будет включена служба камер Windows и добавлено устройство «Android Tools Camera».",
      "Установить", "primary");
    if (!ok) return;
    toast("Установка камеры…");
    await call("camera_install");
    toast("Android Tools Camera установлена", "ok");
    setTimeout(() => this.checkVcam(), 800);
  },
  async remove() {
    if (!(await confirmBox("Удалить камеру?", "Устройство «Android Tools Camera» исчезнет из системы.", "Удалить"))) return;
    await call("camera_remove");
    this.setRunning(false);
    toast("Камера удалена", "ok");
    this.checkVcam();
  },
  async start() {
    const serial = requireDevice();
    if (!serial) return;
    if (!S.info[serial]) await refreshInfo(serial);
    if (this.vcam && this.vcam.supported && !this.vcam.device) toast("Камера Windows не установлена — будет только превью");
    const c = cfg.camera;
    this.setRunning(true);
    $("#camStatus").textContent = "Подключение…";
    await call("camera_start", {
      settings: {
        serial, sdk: S.info[serial]?.sdk || 0, facing: c.facing, camera_id: c.camera_id, quality: c.quality,
        fps: c.fps, bitrate: c.bitrate, mode: c.mode, torch: c.torch, live: this.live(),
      },
    }).catch(() => this.setRunning(false));
  },
  async stop() {
    this.setRunning(false);
    await call("camera_stop", {}, { busy: false, quiet: true });
  },
  clearPreview(text = IS_WINDOWS ? "Нет сигнала" : "Превью недоступно в Linux — откройте камеру в любой программе") {
    const box = $("#camPreview");
    box.classList.remove("live");
    $("img", box).removeAttribute("src");
    $("#camPh").textContent = text;
  },
  setRunning(on) {
    this.running = on;
    const b = $("#camStart");
    b.className = `btn big block ${on ? "stop" : "primary"}`;
    b.innerHTML = on ? `<i class="ic">&#xE71A;</i><span>Выключить</span>` : `<i class="ic">&#xE722;</i><span>Включить камеру</span>`;
    if (!on) {
      this.clearPreview();
      $("#camStatus").textContent = "Выключено";
      $("#camDot").classList.remove("on");
      $("#camFps").textContent = "";
    }
  },
});

/* ═════════════ APPS ═════════════ */

registerPage("apps", {
  items: [],
  selected: new Set(),
  anchor: null,
  loadedFor: null,
  installing: false,
  init(el, actions) {
    bindAll(actions, () => this.refresh());
    $("#appSearch", actions).oninput = () => this.render();
    $("#appInstall", actions).onclick = async () => {
      const files = await T.dialog.open({
        multiple: true, title: "APK",
        filters: [{ name: "Android", extensions: ["apk", "apks", "xapk", "apkm"] }],
      });
      if (files?.length) this.install(files);
    };
    for (const b of $$("[data-app]", el)) b.onclick = () => this.action(b.dataset.app);
    const list = $("#appList");
    list.addEventListener("click", (e) => this.click(e));
    list.addEventListener("dblclick", (e) => e.target.closest(".item") && this.action("launch"));
    list.addEventListener("contextmenu", (e) => {
      const row = e.target.closest(".item");
      if (!row) return;
      if (!this.selected.has(row.dataset.pkg)) { this.selected = new Set([row.dataset.pkg]); this.paintSel(); }
      const one = this.selected.size === 1;
      menu(e.clientX, e.clientY, [
        { label: "Открыть", disabled: !one, action: () => this.action("launch") },
        { label: "Остановить", action: () => this.action("stop") },
        { label: "Сохранить APK", action: () => this.action("extract") },
        { label: "Копировать имя пакета", action: () => navigator.clipboard.writeText([...this.selected].join("\n")) },
        "-",
        { label: "Удалить", danger: true, action: () => this.action("uninstall") },
      ]);
    });
    onDevice(() => current === "apps" && this.show());
  },
  show() {
    if (online() && this.loadedFor !== S.serial + cfg.apps.filter) this.refresh();
    else if (!online()) this.empty("Нет устройства", "Подключите телефон на вкладке «Девайсы»");
  },
  empty(title, hint) {
    this.items = [];
    this.selected.clear();
    $("#appList").innerHTML = `<div class="empty"><i class="ic">&#xE71D;</i><div class="big">${title}</div><div class="hint">${hint}</div></div>`;
    $("#appCount").textContent = "";
    this.paintSel();
  },
  async refresh() {
    const serial = requireDevice();
    if (!serial) return this.empty("Нет устройства", "Подключите телефон на вкладке «Девайсы»");
    const kind = cfg.apps.filter;
    this.items = await call("apps_list", { serial, kind });
    this.loadedFor = serial + kind;
    const names = new Set(this.items.map((a) => a.package));
    this.selected = new Set([...this.selected].filter((p) => names.has(p)));
    this.render();
  },
  visible() {
    const q = ($("#appSearch", this.actions)?.value || "").trim().toLowerCase();
    return q ? this.items.filter((a) => a.package.toLowerCase().includes(q)) : this.items;
  },
  render() {
    const shown = this.visible();
    const list = $("#appList");
    list.innerHTML = shown.map((a) => `
      <div class="item${a.disabled ? " off" : ""}${this.selected.has(a.package) ? " sel" : ""}" data-pkg="${esc(a.package)}">
        <i class="ic">${a.disabled ? "&#xF140;" : "&#xE7B8;"}</i><span class="name">${esc(a.package)}</span>
        ${a.disabled ? `<span class="meta">отключено</span>` : ""}
      </div>`).join("") || `<div class="empty"><span class="hint">Ничего не найдено</span></div>`;
    const total = this.items.length;
    $("#appCount").textContent = shown.length !== total ? `${shown.length} из ${total}` : plural(total, "приложение", "приложения", "приложений");
    this.paintSel();
  },
  click(e) {
    const row = e.target.closest(".item");
    if (!row) return;
    const pkg = row.dataset.pkg;
    const order = this.visible().map((a) => a.package);
    if (e.shiftKey && this.anchor) {
      const [a, b] = [order.indexOf(this.anchor), order.indexOf(pkg)].sort((x, y) => x - y);
      this.selected = new Set(order.slice(a, b + 1));
    } else if (e.ctrlKey) {
      this.selected.has(pkg) ? this.selected.delete(pkg) : this.selected.add(pkg);
      this.anchor = pkg;
    } else {
      this.selected = new Set([pkg]);
      this.anchor = pkg;
    }
    this.paintSel();
  },
  paintSel() {
    for (const r of $$("#appList .item")) r.classList.toggle("sel", this.selected.has(r.dataset.pkg));
    const pkgs = [...this.selected];
    $("#appNone").style.display = pkgs.length ? "none" : "";
    $("#appDetails").style.display = pkgs.length ? "" : "none";
    if (!pkgs.length) return;
    const one = pkgs.length === 1;
    for (const k of ["launch", "clear", "toggle"]) $(`[data-app=${k}]`).style.display = one ? "" : "none";
    if (!one) {
      $("#appName").textContent = `Выбрано: ${pkgs.length}`;
      $("#appMeta").textContent = "Действия применятся ко всем";
      return;
    }
    const app = this.items.find((a) => a.package === pkgs[0]) || {};
    $("#appName").textContent = pkgs[0];
    $("#appMeta").textContent = "…";
    $("[data-app=toggle] span").textContent = app.disabled ? "Включить" : "Отключить";
    $("[data-app=launch]").disabled = !!app.disabled;
    call("app_details", { serial: S.serial, package: pkgs[0] }, { busy: false, quiet: true }).then((d) => {
      if (this.selected.size !== 1 || !this.selected.has(pkgs[0])) return;
      $("#appMeta").textContent = [
        d.version && `Версия  ${d.version}`, d.target_sdk && `SDK  ${d.target_sdk}`,
        d.installed && `Установлено  ${d.installed.slice(0, 10)}`, d.updated && `Обновлено  ${d.updated.slice(0, 10)}`,
      ].filter(Boolean).join("\n");
    }).catch(() => ($("#appMeta").textContent = ""));
  },
  async action(kind) {
    const serial = requireDevice();
    const pkgs = [...this.selected];
    if (!serial || !pkgs.length) return;
    const what = pkgs.length === 1 ? pkgs[0] : plural(pkgs.length, "приложение", "приложения", "приложений");
    const run = (action) => call("app_action", { serial, packages: pkgs, action, saveDir: cfg.save_dir });
    switch (kind) {
      case "launch": await run("launch"); return toast("Запущено", "ok");
      case "stop": await run("stop"); return toast("Остановлено", "ok");
      case "clear":
        if (!(await confirmBox("Очистить данные?", `${what}: настройки, вход и кэш будут удалены.`, "Очистить"))) return;
        await run("clear"); return toast("Данные очищены", "ok");
      case "toggle": {
        const app = this.items.find((a) => a.package === pkgs[0]);
        if (!app.disabled && !(await confirmBox("Отключить?", `${what} исчезнет из меню и не будет запускаться.`, "Отключить"))) return;
        await run(app.disabled ? "enable" : "disable");
        app.disabled = !app.disabled;
        toast(app.disabled ? "Отключено" : "Включено", "ok");
        return this.render();
      }
      case "extract": {
        await run("extract");
        toast(`Сохранено: ${what}`, "ok");
        return call("open_path", { path: saveSub("APK") }, { busy: false });
      }
      case "uninstall":
        if (!(await confirmBox("Удалить?", `${what} будет удалено с телефона.`, "Удалить"))) return;
        await run("uninstall");
        toast(`Удалено: ${what}`, "ok");
        this.items = this.items.filter((a) => !this.selected.has(a.package));
        this.selected.clear();
        return this.render();
    }
  },
  async install(files) {
    const serial = requireDevice();
    if (!serial) return;
    if (this.installing) return toast("Дождитесь окончания установки");
    this.installing = true;
    const name = (f) => f.split(/[\\/]/).pop();
    toast(files.length > 1 ? `Установка: ${files.length}…` : `Установка ${name(files[0])}…`);
    let ok = 0;
    for (const f of files) {
      try {
        await call("app_install", { serial, path: f }, { quiet: true });
        ok++;
        toast(`Установлено: ${name(f)}`, "ok");
      } catch (e) {
        toast(`${name(f)}: ${e}`, "err");
      }
    }
    this.installing = false;
    if (ok && current === "apps") this.refresh();
  },
});

/* ═════════════ FILES ═════════════ */

const PLACES = [
  ["Память", "/sdcard/"], ["Загрузки", "/sdcard/Download/"], ["Камера", "/sdcard/DCIM/"], ["Фото", "/sdcard/Pictures/"],
  ["Видео", "/sdcard/Movies/"], ["Музыка", "/sdcard/Music/"], ["Документы", "/sdcard/Documents/"],
];
const fileIcon = (name) => {
  const ext = name.split(".").pop().toLowerCase();
  if (["jpg", "jpeg", "png", "webp", "gif", "heic", "bmp"].includes(ext)) return "&#xEB9F;";
  if (["mp4", "mkv", "mov", "avi", "webm", "3gp"].includes(ext)) return "&#xE714;";
  if (["mp3", "m4a", "ogg", "wav", "flac", "opus", "aac"].includes(ext)) return "&#xE8D6;";
  if (["apk", "apks", "xapk"].includes(ext)) return "&#xE7B8;";
  if (["zip", "rar", "7z", "tar", "gz"].includes(ext)) return "&#xF012;";
  return "&#xE8A5;";
};

registerPage("files", {
  items: [],
  selected: new Set(),
  history: [],
  loadedFor: null,
  init(el, actions) {
    $("#filePlaces").innerHTML = PLACES.map(([n, p]) => `<button class="btn small" data-path="${p}">${n}</button>`).join("");
    $("#filePlaces").onclick = (e) => e.target.dataset.path && this.open(e.target.dataset.path);
    $("#fileBack").onclick = () => this.history.length && this.open(this.history.pop(), false);
    $("#fileUp").onclick = () => {
      const p = cfg.files.path.replace(/\/+$/, "");
      if (p) this.open(p.slice(0, p.lastIndexOf("/") + 1) || "/");
    };
    $("#fileMkdir", actions).onclick = () => this.mkdir();
    $("#fileUpload", actions).onclick = async () => {
      const files = await T.dialog.open({ multiple: true, title: "Загрузить на телефон" });
      if (files?.length) this.upload(files);
    };
    $("#fileDownload", actions).onclick = () => this.download([...this.selected]);
    const list = $("#fileList");
    list.addEventListener("click", (e) => {
      const row = e.target.closest(".item");
      if (!row) return;
      const n = row.dataset.name;
      if (e.ctrlKey) this.selected.has(n) ? this.selected.delete(n) : this.selected.add(n);
      else this.selected = new Set([n]);
      this.paintSel();
    });
    list.addEventListener("dblclick", (e) => {
      const row = e.target.closest(".item");
      if (!row) return;
      const it = this.items.find((i) => i.name === row.dataset.name);
      if (it.dir) this.open(cfg.files.path + it.name + "/");
      else this.download([it.name], true);
    });
    list.addEventListener("contextmenu", (e) => {
      const row = e.target.closest(".item");
      if (!row) {
        return menu(e.clientX, e.clientY, [
          { label: "Новая папка", action: () => this.mkdir() },
          { label: "Обновить", action: () => this.refresh() },
        ]);
      }
      if (!this.selected.has(row.dataset.name)) { this.selected = new Set([row.dataset.name]); this.paintSel(); }
      const sel = [...this.selected];
      const it = this.items.find((i) => i.name === sel[0]);
      const one = sel.length === 1;
      menu(e.clientX, e.clientY, [
        one && it.dir ? { label: "Открыть", action: () => this.open(cfg.files.path + it.name + "/") }
          : { label: "Открыть на ПК", disabled: !one, action: () => this.download(sel, true) },
        { label: "Скачать…", action: () => this.download(sel) },
        { label: "Переименовать", disabled: !one, action: () => this.rename(sel[0]) },
        { label: "Копировать путь", action: () => navigator.clipboard.writeText(sel.map((n) => cfg.files.path + n).join("\n")) },
        "-",
        { label: "Удалить", danger: true, action: () => this.remove(sel) },
      ]);
    });
    this.crumbs();
    onDevice(() => current === "files" && this.show());
  },
  show() {
    if (!online()) {
      $("#fileList").innerHTML = `<div class="empty"><i class="ic">&#xE8B7;</i><div class="big">Нет устройства</div><div class="hint">Подключите телефон на вкладке «Девайсы»</div></div>`;
      $("#fileCount").textContent = "";
      return;
    }
    if (this.loadedFor !== S.serial) this.refresh();
  },
  open(path, remember = true) {
    if (!path.endsWith("/")) path += "/";
    if (remember && path !== cfg.files.path) this.history.push(cfg.files.path);
    cfg.files.path = path; save();
    this.refresh();
  },
  crumbs() {
    const parts = cfg.files.path.split("/").filter(Boolean);
    let acc = "/";
    const html = [`<button class="btn ghost small" data-path="/">/</button>`];
    parts.forEach((p, i) => {
      acc += p + "/";
      if (i) html.push(`<span class="sep">›</span>`);
      html.push(`<button class="btn ${i === parts.length - 1 ? "" : "ghost"} small" data-path="${esc(acc)}">${esc(p)}</button>`);
    });
    const c = $("#fileCrumbs");
    c.innerHTML = html.join("");
    c.onclick = (e) => { const b = e.target.closest("[data-path]"); if (b) this.open(b.dataset.path); };
  },
  async refresh() {
    const serial = requireDevice();
    if (!serial) return;
    const path = cfg.files.path;
    this.crumbs();
    const items = await call("files_list", { serial, path });
    if (path !== cfg.files.path) return;
    this.items = items;
    this.loadedFor = serial;
    this.selected.clear();
    this.render();
  },
  render() {
    const fmt = (t) => t ? new Date(t * 1000).toLocaleString(I18N.locale, { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" }).replace(",", "") : "";
    const rows = this.items.map((it) => `
      <div class="item" data-name="${esc(it.name)}">
        <i class="ic ${it.dir ? "folder" : ""}">${it.dir ? "&#xE8B7;" : fileIcon(it.name)}</i>
        <span class="name">${esc(it.name)}</span>
        <span class="meta" style="width:90px">${it.dir ? "" : humanSize(it.size)}</span>
        <span class="meta" style="width:130px">${fmt(it.mtime)}</span>
      </div>`).join("");
    $("#fileList").innerHTML = `<div class="list-head"><span style="width:18px"></span><span class="grow">ИМЯ</span>
      <span style="width:90px;text-align:right">РАЗМЕР</span><span style="width:130px;text-align:right">ИЗМЕНЁН</span></div>` +
      (rows || `<div class="empty"><i class="ic">&#xE8B7;</i><div class="big">Пусто</div><div class="hint">Папка пуста или нет доступа</div></div>`);
    const dirs = this.items.filter((i) => i.dir).length;
    $("#fileCount").textContent = `Папок: ${dirs}  ·  Файлов: ${this.items.length - dirs}`;
    this.paintSel();
  },
  paintSel() {
    for (const r of $$("#fileList .item")) r.classList.toggle("sel", this.selected.has(r.dataset.name));
    const b = $("#fileDownload", this.actions);
    if (b) b.disabled = !this.selected.size;
  },
  async download(names, openAfter = false) {
    const serial = requireDevice();
    if (!serial || !names.length) return;
    let dest = saveSub("Файлы");
    if (!openAfter) {
      const chosen = await T.dialog.open({ directory: true, title: "Куда сохранить", defaultPath: cfg.save_dir });
      if (!chosen) return;
      dest = chosen;
    }
    toast(`Скачивание: ${names.length}…`);
    await call("files_pull", { serial, remote: names.map((n) => cfg.files.path + n), dest });
    if (openAfter && names.length === 1) await call("open_path", { path: joinPath(dest, names[0]) }, { busy: false });
    else { toast(`Сохранено в ${dest}`, "ok"); call("open_path", { path: dest }, { busy: false }); }
  },
  async upload(files) {
    const serial = requireDevice();
    if (!serial) return;
    const dest = cfg.files.path;
    toast(`Загрузка: ${files.length} → ${dest}`);
    await call("files_push", { serial, files, dest });
    toast("Загружено", "ok");
    if (cfg.files.path === dest && current === "files") this.refresh();
  },
  async mkdir() {
    const serial = requireDevice();
    if (!serial) return;
    const name = await prompt("Новая папка", "", "Имя");
    if (!name) return;
    await call("files_op", { serial, op: "mkdir", path: cfg.files.path + name, target: null });
    this.refresh();
  },
  async rename(old) {
    const serial = requireDevice();
    const name = await prompt("Переименовать", old);
    if (!serial || !name || name === old) return;
    await call("files_op", { serial, op: "rename", path: cfg.files.path + old, target: cfg.files.path + name });
    this.refresh();
  },
  async remove(names) {
    const serial = requireDevice();
    if (!serial) return;
    const what = names.length === 1 ? names[0] : plural(names.length, "объект", "объекта", "объектов");
    if (!(await confirmBox("Удалить?", `${what} — без возможности восстановления.`, "Удалить"))) return;
    for (const n of names) await call("files_op", { serial, op: "delete", path: cfg.files.path + n, target: null });
    toast("Удалено", "ok");
    this.refresh();
  },
});

/* ═════════════ UTILS ═════════════ */

registerPage("utils", {
  history: [],
  cursor: 0,
  lastShot: null,
  init(el) {
    bindAll(el);
    $("#shotTake").onclick = async () => {
      const serial = requireDevice();
      if (!serial) return;
      const r = await call("screenshot", { serial, saveDir: cfg.save_dir, clipboard: cfg.utils.clipboard });
      this.lastShot = r.path;
      $("#shotThumb").innerHTML = `<img src="${r.data}" alt="">`;
      toast("Скриншот сохранён" + (r.copied ? " и скопирован" : ""), "ok");
    };
    $("#shotThumb").onclick = () => this.lastShot && call("open_path", { path: this.lastShot }, { busy: false });
    for (const b of $$("[data-key]", el)) {
      b.onclick = () => { const s = requireDevice(); if (s) call("key_event", { serial: s, key: b.dataset.key }, { busy: false }); };
    }
    for (const b of $$("[data-reboot]", el)) {
      b.onclick = async () => {
        const s = requireDevice();
        if (!s) return;
        const off = b.dataset.reboot === "off";
        const ok = off
          ? await confirmBox("Выключить телефон?", "Включить обратно можно только кнопкой питания.", "Выключить")
          : await confirmBox("Перезагрузить?", `Режим: ${b.dataset.name}`, "Перезагрузить", "primary");
        if (!ok) return;
        await call("reboot", { serial: s, mode: b.dataset.reboot });
        toast(off ? "Выключение…" : "Перезагрузка…", "ok");
      };
    }
    const input = $("#sendText");
    const sendType = async () => {
      const s = requireDevice();
      if (s && input.value) { await call("send_text", { serial: s, text: input.value }); toast("Отправлено", "ok"); }
    };
    const sendUrl = async () => {
      const s = requireDevice();
      if (s && input.value.trim()) { await call("open_url", { serial: s, url: input.value.trim() }); toast("Открыто на телефоне", "ok"); }
    };
    $("#sendType").onclick = sendType;
    $("#sendUrl").onclick = sendUrl;
    input.onkeydown = (e) => e.key === "Enter" && (/^(https?:\/\/|www\.)/i.test(input.value.trim()) ? sendUrl() : sendType());

    const out = $("#shellOut");
    const shell = $("#shellIn");
    $("#shellClear").onclick = () => (out.innerHTML = "");
    shell.onkeydown = async (e) => {
      if (e.key === "ArrowUp" && this.history.length) {
        this.cursor = Math.max(0, this.cursor - 1); shell.value = this.history[this.cursor]; e.preventDefault();
      } else if (e.key === "ArrowDown" && this.history.length) {
        this.cursor = Math.min(this.history.length, this.cursor + 1); shell.value = this.history[this.cursor] || "";
      } else if (e.key === "Enter") {
        const cmd = shell.value.trim();
        const s = requireDevice();
        if (!cmd || !s) return;
        if (this.history.at(-1) !== cmd) this.history.push(cmd);
        this.cursor = this.history.length;
        shell.value = "";
        if (out.querySelector(".faint")) out.innerHTML = "";
        out.insertAdjacentHTML("beforeend", `<div class="ok">$ ${esc(cmd)}</div>`);
        const res = await call("shell", { serial: s, command: cmd }).catch((err) => String(err));
        out.insertAdjacentHTML("beforeend", `<div>${esc(res.trimEnd() || "(пусто)")}</div><br>`);
        out.scrollTop = out.scrollHeight;
      }
    };
  },
});

/* ═════════════ SETTINGS ═════════════ */

registerPage("settings", {
  init(el) {
    bindAll(el, (v, path) => {
      if (path === "lang") location.reload();
    });
    $("#toolsDir").onclick = () => this.tools && call("open_path", { path: this.tools.bin_dir }, { busy: false });
    $("#saveDirOpen").onclick = () => call("open_path", { path: cfg.save_dir }, { busy: false });
    $("#saveDirPick").onclick = async () => {
      const d = await T.dialog.open({ directory: true, title: "Папка сохранения", defaultPath: cfg.save_dir });
      if (d) { cfg.save_dir = d; save(); this.paint(); }
    };
    const update = async (name, btn) => {
      const b = $(btn);
      b.disabled = true;
      $("span", b).textContent = "Загрузка…";
      try {
        const v = await call("update_tool", { name });
        toast(`${name === "adb" ? "Platform Tools" : "scrcpy"}: v${v}`, "ok");
        if (name === "adb") { S.signature = null; poll(true); }
      } finally {
        b.disabled = false;
        $("span", b).textContent = "Обновить";
        this.show();
      }
    };
    $("#adbUpdate").onclick = () => update("adb", "#adbUpdate");
    $("#scrcpyUpdate").onclick = () => update("scrcpy", "#scrcpyUpdate");
  },
  async show() {
    this.paint();
    const t = await call("tools_info", {}, { busy: false, quiet: true }).catch(() => null);
    if (!t) return;
    this.tools = t;
    $("#adbVer").textContent = t.adb_version ? `v${t.adb_version}` : "не найден";
    $("#adbPath").textContent = shortPath(t.adb_path);
    $("#scrcpyVer").textContent = t.scrcpy_version ? `v${t.scrcpy_version}` : "не найден";
    $("#scrcpyPath").textContent = shortPath(t.scrcpy_path);
    for (const id of ["#adbUpdate", "#scrcpyUpdate"]) $(id).style.display = t.managed ? "none" : "";
    $("#toolsNote").textContent = t.managed
      ? "Обновляется через pacman"
      : "Встроены в программу и распаковываются при первом запуске. «Обновить» скачивает последние версии.";
  },
  paint() { $("#saveDir").textContent = shortPath(cfg.save_dir); },
});

/* ═════════════ shell: pill, shortcuts, drag & drop, boot ═════════════ */

function paintPill() {
  const d = device();
  const pill = $("#pill");
  $(".dot", pill).style.background = !d ? "var(--faint)" : d.state === "device" ? "var(--accent)" : d.state === "unauthorized" ? "var(--warn)" : "var(--danger)";
  $(".name", pill).textContent = d ? devName() : "Нет устройства";
}
$("#pill").onclick = (e) => {
  const r = $("#pill").getBoundingClientRect();
  const items = S.devices.map((d) => ({
    label: `${devName(d.serial)}  ·  ${d.wireless ? "Wi‑Fi" : "USB"}`,
    checked: d.serial === S.serial,
    disabled: d.state !== "device",
    action: () => select(d.serial),
  }));
  if (!items.length) items.push({ label: "Нет подключённых", disabled: true });
  items.push("-", { label: "Все устройства…", action: () => go("devices") });
  menu(r.left, r.bottom + 6, items);
};
onDevice(paintPill);

for (const n of $$(".nav")) n.onclick = () => go(n.dataset.page);
for (const b of $$("[data-open-dir]")) b.onclick = () => call("open_path", { path: saveSub(b.dataset.openDir) }, { busy: false });

document.addEventListener("keydown", (e) => {
  if ($("#modalBack").classList.contains("show")) return;
  const order = ["devices", "screen", "camera", "apps", "files", "utils", "settings"];
  if (e.ctrlKey && e.key >= "1" && e.key <= "7") { go(order[+e.key - 1]); e.preventDefault(); }
  if (e.key === "F5") { pages[current]?.refresh?.(); e.preventDefault(); }
  if (e.key === "Escape") hideMenu();
});

const APK = /\.(apk|apks|xapk|apkm)$/i;
T.webviewWindow.getCurrentWebviewWindow().onDragDropEvent((e) => {
  const drop = $("#drop");
  const p = e.payload;
  if (!["apps", "files"].includes(current)) return;
  if (p.type === "enter") {
    const ok = current === "files" ? p.paths.length : p.paths.some((f) => APK.test(f));
    drop.textContent = current === "files" ? "Отпустите, чтобы загрузить сюда" : "Отпустите, чтобы установить";
    drop.classList.toggle("show", !!ok);
  } else if (p.type === "leave") {
    drop.classList.remove("show");
  } else if (p.type === "drop") {
    drop.classList.remove("show");
    if (current === "apps") {
      const apks = p.paths.filter((f) => APK.test(f));
      if (apks.length) pages.apps.install(apks);
    } else if (p.paths.length) {
      pages.files.upload(p.paths);
    }
  }
});

// First launch: pick the interface language (remembered in cfg.lang).
function chooseLanguage() {
  return new Promise((resolve) => {
    const back = document.createElement("div");
    back.className = "modal-back show";
    back.innerHTML = `
      <div class="modal lang-pick">
        <i class="ic" style="font-size:30px;color:var(--accent)">&#xE8EA;</i>
        <h3>Android Tools</h3>
        <div class="hint">Выберите язык · Choose your language</div>
        <div class="buttons" style="justify-content:stretch;margin-top:18px">
          <button class="btn big grow" data-lang="ru">Русский</button>
          <button class="btn big grow" data-lang="en">English</button>
        </div>
      </div>`;
    back.onclick = (e) => {
      const b = e.target.closest("[data-lang]");
      if (!b) return;
      back.remove();
      resolve(b.dataset.lang);
    };
    document.body.append(back);
  });
}

(async function boot() {
  if (!cfg.lang) {
    cfg.lang = await chooseLanguage();
    save();
    repaint($("#page-settings"));
  }
  I18N.apply(cfg.lang);
  call("set_language", { lang: cfg.lang }, { busy: false, quiet: true }).catch(() => {});
  go("devices");
  try {
    const info = await call("app_info", {}, { quiet: true });
    if (!cfg.save_dir) { cfg.save_dir = info.save_dir; save(); }
    $("#aboutVer").textContent = `Версия ${info.version}  ·  Tauri · adb · scrcpy`;
  } catch (e) {
    toast(`Инструменты: ${e}`, "err");
  }
  await poll(true);
  setInterval(() => poll(), 2500);
})();
