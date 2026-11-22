"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;

// the same script drives the island (index.html) and the desktop manager window (manager.html)
const MANAGER = document.body.dataset.mode === "manager";
const MAX_CONNS_PER_APP = 60;
// script errors go to the app log (release builds have no devtools)
window.addEventListener("error", (e) => invoke("log_js", { msg: `${MANAGER ? "manager" : "island"} error: ${e.message} @${e.lineno}:${e.colno}` }).catch(() => {}));
window.addEventListener("unhandledrejection", (e) => invoke("log_js", { msg: `${MANAGER ? "manager" : "island"} rejection: ${e.reason}` }).catch(() => {}));

const $ = (id) => document.getElementById(id);
const island = $("island");
const pill = $("pill");

let snap = null;
let exitState = null;
let expanded = false;
let tab = "conns";
let query = "";
const openApps = new Set();
const showAll = new Set();
const icons = new Map(); // path -> dataURL | null | "pending"
if (MANAGER) {
  expanded = true; // the manager window always shows the "panel"
  tab = decodeURIComponent(location.hash.slice(1)) || "home";
  // "settings/notify": a settings category (picked up where the category is first read)
  const [page, cat] = tab.split("/");
  if (cat) {
    tab = page;
    try { localStorage.setItem("mb-set-cat", cat); } catch { /* ignore */ }
  }
}

let regionNames = null;
try { regionNames = new Intl.DisplayNames(["zh-CN"], { type: "region" }); } catch { /* ignore */ }

// ---------- settings ----------

let S = null; // settings document from the backend
const sget = (path, dflt) => path.split(".").reduce((o, k) => (o == null ? o : o[k]), S) ?? dflt;
const itemOn = (listPath, id) => (sget(listPath, []) || []).some((i) => i.id === id && i.on);

// island sizes (CSS px); while the layout is being edited the live values come from editSize
const SIZE_LIMITS = { pillH: [28, 64], pillW: [0, 1400], pillMaxW: [200, 1400], panelW: [480, 1100], panelH: [360, 900] };
const SIZE_DEFAULTS = { pillH: 44, pillW: 0, pillMaxW: 760, panelW: 600, panelH: 540 };
let editSize = null;
let editing = false; // layout editing (drag handles)
let editPart = "panel";
const sz = (k) => {
  const v = editSize && editSize[k] != null ? editSize[k] : sget("size." + k, SIZE_DEFAULTS[k]);
  const [lo, hi] = SIZE_LIMITS[k];
  return Math.min(hi, Math.max(lo, Number(v) || SIZE_DEFAULTS[k]));
};
const panelW = () => sz("panelW");
const panelH = () => sz("panelH");

// ---------- theme (white / black frosted glass) ----------

const PRESETS = {
  onedark: { name: "One Dark Pro", base: "dark", bg: "#282c34", text: "#abb2bf", dim: "#8b93a1", dimmer: "#5c6370", blue: "#61afef", green: "#98c379", amber: "#e5c07b", red: "#e06c75", violet: "#c678dd" },
  "github-dark": { name: "GitHub 深色", base: "dark", bg: "#0d1117", text: "#e6edf3", dim: "#9da7b3", dimmer: "#6e7681", blue: "#58a6ff", green: "#3fb950", amber: "#d29922", red: "#f85149", violet: "#bc8cff" },
  "github-light": { name: "GitHub 浅色", base: "light", bg: "#ffffff", text: "#1f2328", dim: "#59636e", dimmer: "#818b98", blue: "#0969da", green: "#1a7f37", amber: "#9a6700", red: "#d1242f", violet: "#8250df" },
  dracula: { name: "Dracula", base: "dark", bg: "#282a36", text: "#f8f8f2", dim: "#b6b9c8", dimmer: "#6272a4", blue: "#8be9fd", green: "#50fa7b", amber: "#f1fa8c", red: "#ff5555", violet: "#bd93f9" },
  nightowl: { name: "Night Owl", base: "dark", bg: "#011627", text: "#d6deeb", dim: "#a1aab8", dimmer: "#637777", blue: "#82aaff", green: "#addb67", amber: "#ecc48d", red: "#ef5350", violet: "#c792ea" },
  "solarized-light": { name: "Solarized 浅色", base: "light", bg: "#fdf6e3", text: "#073642", dim: "#586e75", dimmer: "#93a1a1", blue: "#268bd2", green: "#859900", amber: "#b58900", red: "#dc322f", violet: "#6c71c4" },
};
const THEME_VARS = ["--tint-rgb", "--solid-rgb", "--text", "--dim", "--dimmer", "--glass-dim", "--glass-dimmer", "--blue", "--green", "--amber", "--red", "--violet", "--menu-bg", "--toast-bg", "--font-ui", "--bg-a"];

const hexRgb = (h) => {
  const m = String(h || "").replace("#", "").match(/^([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : [22, 22, 28];
};
const mixHex = (a, b, t) => {
  const [x, y] = [hexRgb(a), hexRgb(b)];
  return "#" + x.map((v, i) => Math.round(v + (y[i] - v) * t).toString(16).padStart(2, "0")).join("");
};

let glassOpacity = 50; // appearance.opacity, used by adaptTint

const lightQuery = matchMedia("(prefers-color-scheme: light)");
function applyTheme() {
  const pref = sget("appearance.theme", "auto");
  const root = document.documentElement.style;
  THEME_VARS.forEach((v) => root.removeProperty(v));
  let base, t = null;
  if (pref === "glass-light") base = "light";
  else if (pref === "glass-dark") base = "dark";
  else if (PRESETS[pref]) { t = PRESETS[pref]; base = t.base; }
  else if (pref === "custom") {
    const c = sget("appearance.custom", {}) || {};
    base = c.base === "light" ? "light" : "dark";
    t = { bg: c.bg || "#16161c", text: c.text || "#f2f2f5", blue: c.accent || "#60a5fa", violet: c.accent || "#60a5fa" };
    t.dim = mixHex(t.text, t.bg, 0.3);
    t.dimmer = mixHex(t.text, t.bg, 0.5);
  } else base = lightQuery.matches ? "light" : "dark";
  document.documentElement.dataset.theme = base;
  if (t) {
    const rgb = hexRgb(t.bg).join(", ");
    root.setProperty("--tint-rgb", rgb);
    root.setProperty("--solid-rgb", rgb);
    root.setProperty("--menu-bg", `rgba(${rgb}, 0.98)`);
    root.setProperty("--toast-bg", `rgba(${rgb}, 0.97)`);
    for (const k of ["text", "dim", "dimmer", "blue", "green", "amber", "red", "violet"]) if (t[k]) root.setProperty("--" + k, t[k]);
    if (t.dim) root.setProperty("--glass-dim", t.dim);
    if (t.dimmer) root.setProperty("--glass-dimmer", t.dimmer);
  }
  glassOpacity = sget("appearance.opacity", 50);
  // the collapsed pill as a whole (text included); the open panel stays opaque
  root.setProperty("--pill-op", String(Math.min(100, Math.max(10, Number(sget("appearance.pillOpacity", 100)) || 100)) / 100));
  // solid background (glass off): 50 -> 0.97, 100 -> opaque, 20 -> 0.7
  root.setProperty("--bg-a", String(Math.min(1, 0.52 + (0.9 * glassOpacity) / 100)));
  const font = sget("appearance.font", "");
  if (font) root.setProperty("--font-ui", `"${font.replace(/"/g, "")}", "Segoe UI", "Microsoft YaHei UI", sans-serif`);
}
lightQuery.addEventListener("change", applyTheme);
applyTheme();

// ---------- helpers ----------

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function flagImg(cc, cls = "") {
  if (!cc || !/^[A-Za-z]{2}$/.test(cc)) return "";
  const c = cc.toLowerCase();
  return `<img class="flag ${cls}" src="flags/${c}.svg" alt="${esc(cc)}" onerror="this.remove()">`;
}

function country(cc) {
  if (!cc) return "";
  try { return regionNames ? regionNames.of(cc.toUpperCase()) : cc; } catch { return cc; }
}

// Windows can't draw flag emoji: turn regional-indicator pairs (e.g. in node names) into flag images.
function decorate(text) {
  const s = esc(text);
  return s.replace(/([\u{1F1E6}-\u{1F1FF}])([\u{1F1E6}-\u{1F1FF}])/gu, (_, a, b) => {
    const cc = String.fromCharCode(a.codePointAt(0) - 0x1f1e6 + 65, b.codePointAt(0) - 0x1f1e6 + 65);
    return flagImg(cc, "sm");
  });
}

function fmtRate(b) {
  if (!b || b < 1) return "0";
  if (b < 1024) return `${b.toFixed(0)} B/s`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(b < 10240 ? 1 : 0)} KB/s`;
  return `${(b / 1024 / 1024).toFixed(1)} MB/s`;
}

function speedHtml(up, down) {
  if ((!up || up < 1) && (!down || down < 1)) return `<span class="z">—</span>`;
  return `<span class="u">↑</span>${fmtRate(up)} <span class="d">↓</span>${fmtRate(down)}`;
}

function ago(ms) {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 10) return "刚刚";
  if (s < 60) return `${s} 秒前`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  return `${Math.floor(s / 3600)} 小时前`;
}

function routeClass(node) {
  if (!node) return "";
  const n = node.toUpperCase();
  if (n === "DIRECT") return "direct";
  if (n === "REJECT" || n === "REJECT-DROP") return "reject";
  return "node";
}

// ---------- icons ----------

function iconHtml(app) {
  const p = app.icon || app.path; // helpers borrow their app's icon
  if (p && icons.has(p) && icons.get(p) && icons.get(p) !== "pending") {
    return `<img class="app-icon" src="${icons.get(p)}" alt="">`;
  }
  if (p && !icons.has(p)) {
    icons.set(p, "pending");
    invoke("get_icon", { path: p }).then((url) => {
      icons.set(p, url || null);
      if (!url) setTimeout(() => icons.delete(p), 30_000); // retry later
      if (url) {
        document.querySelectorAll(`[data-icon="${CSS.escape(p)}"]`).forEach((el) => {
          el.outerHTML = `<img class="app-icon" src="${url}" alt="">`;
        });
      }
    }).catch(() => icons.set(p, null));
  }
  const letter = (app.name || app.exe || "?").trim().charAt(0).toUpperCase();
  return `<div class="app-icon letter" data-icon="${esc(p || "")}">${esc(letter)}</div>`;
}

// ---------- island geometry ----------

function pillWidth() {
  // fixed width from settings, else the natural width of the content (up to the max width)
  const fixed = sz("pillW");
  if (fixed > 0) return Math.min(window.innerWidth - 16, Math.max(120, fixed));
  pill.style.width = "";
  const w = pill.scrollWidth;
  return Math.max(120, Math.min(window.innerWidth - 16, sz("pillMaxW"), w));
}

// Window size in CSS px; the backend gets it in physical px (× devicePixelRatio, which also
// carries the Windows "text size" accessibility factor). While editing the layout the window
// grows to the largest allowed panel so the handles can be dragged that far.
let fitted = "";
function fitWindow() {
  if (MANAGER) return;
  const dpr = window.devicePixelRatio || 1;
  const maxW = Math.floor((screen.availWidth || 1920) * 0.98), maxH = Math.floor((screen.availHeight || 1080) * 0.95);
  let w = Math.min(maxW, (editing ? SIZE_LIMITS.panelW[1] : Math.max(panelW(), sz("pillW"), sz("pillMaxW"))) + 40);
  let h = Math.min(maxH, (editing ? SIZE_LIMITS.panelH[1] : panelH()) + 30);
  // the window spans the whole docked edge (transparent and click-through outside the island),
  // so the island can sit at either end or slide between them without the window moving
  if (shape().vertical) h = screen.height;
  else w = screen.width;
  const key = `${w}x${h}@${dpr}`;
  if (key === fitted) return;
  fitted = key;
  invoke("fit_window", { w: Math.round(w * dpr), h: Math.round(h * dpr) }).catch(() => {});
  matchMedia(`(resolution: ${dpr}dppx)`).addEventListener("change", () => { fitted = ""; fitWindow(); }, { once: true });
}

let panelHNow = 540;

// Expanded height follows the content (list rows), up to the panel height setting.
function measurePanel() {
  if (MANAGER) return;
  let h;
  // a bottom sheet rises from the panel's bottom edge: the island stands at full height meanwhile
  if (editing || (expanded && Sheet.isOpen())) h = panelH();
  else {
    const list = $("list");
    const foot = $("foot");
    const inner = $("list-inner");
    const footH = foot.childElementCount ? foot.offsetHeight + 10 : 0;
    const need = list.offsetTop + (inner ? inner.offsetHeight : 120) + footH + 14;
    h = Math.max(240, Math.min(panelH(), Math.ceil(need)));
  }
  if (Math.abs(h - panelHNow) > 3) {
    panelHNow = h;
    if (expanded) applySize();
  }
}

// chart width follows the panel (island) or the content column (manager window)
const chartW = () => Math.max(420, Math.floor((MANAGER ? $("list").clientWidth - 40 : panelW() - 34)));

let maximized = false; // a maximized window is in front (setting: shrink to a line)
// The first time per run that a maximized window turns the pill into the line, the pill says so
// for a few seconds first (it looked like a bug: "I picked the capsule and it became a line").
const maxHint = { shown: false, until: 0 };
// first-run tour (see "first-run tour" below); declared up here because the sizing code reads it
const tour = { on: false, step: 0, h: 0 };
const TOUR_W = 384;

function setMaximized(v) {
  v = !!v;
  if (v === maximized) return;
  maximized = v;
  if (v && !MANAGER && !maxHint.shown && sget("shape.lineWhenMaximized", false) && sget("shape.style", "capsule") !== "line") {
    maxHint.shown = true;
    maxHint.until = Date.now() + 5000;
    fireFlash("maxline", "前台窗口最大化了，岛缩成指示条（设置 → 收起外观里可以关）", "info");
    setTimeout(() => { pillLast = ""; renderPill(); applySize(); }, 5100);
  }
  applySize();
}

// Effective collapsed shape and docked edge.
function shape() {
  const dock = sget("shape.dock", "top");
  let style = sget("shape.style", "capsule");
  if (maximized && sget("shape.lineWhenMaximized", false) && Date.now() >= maxHint.until) style = "line";
  const vertical = dock === "left" || dock === "right";
  if (vertical && style === "capsule") style = "notch"; // a sideways capsule = a notch on that edge
  return { style, dock, vertical };
}

// Island rectangle (CSS px inside the window) for the collapsed or expanded state.
function islandRect(open) {
  const { style, dock, vertical } = shape();
  const W = window.innerWidth, H = window.innerHeight;
  let w, h;
  if (open) {
    w = Math.min(W - 8, panelW());
    h = Math.min(H - 8, panelHNow);
  } else if (tour.on && !nst.on && !fling.active) {
    // the tour's card (any shape / edge)
    w = Math.min(W - 16, TOUR_W);
    h = Math.min(H - 16, Math.max(sz("pillH"), tour.h || 150));
  } else if (nst.on) {
    // a notification card (any shape / edge)
    w = Math.min(W - 16, NOTE_W);
    h = Math.min(H - 16, Math.max(sz("pillH"), nst.h || 64));
  } else if (style === "line") {
    [w, h] = vertical ? [6, 140] : [140, 6];
  } else if (vertical) {
    w = sz("pillH") + 4;
    h = Math.max(60, Math.min(H - 16, pill.scrollHeight));
  } else {
    w = pillWidth();
    h = sz("pillH");
  }
  // capsule floats 6px off the edge; notch and line sit on it; an expanded panel floats unless notch
  const card = nst.on || (tour.on && !fling.active);
  const gap = open ? (style === "notch" ? 0 : 6) : style === "capsule" || (card && style === "line") ? 6 : 0;
  let x = (W - w) / 2, y = (H - h) / 2;
  if (dock === "top") y = gap;
  else if (dock === "bottom") y = H - h - gap;
  else if (dock === "left") x = gap;
  else x = W - w - gap;
  // where along the edge (set by flinging it): the window sits at that end, the island too
  const anchor = sget("shape.anchor", "center");
  if (anchor === "start" || anchor === "end") {
    if (vertical) y = anchor === "start" ? EDGE_M : H - h - EDGE_M;
    else x = anchor === "start" ? EDGE_M : W - w - EDGE_M;
  }
  // carried off the edge: centred on the pointer (the window covers the whole monitor meanwhile)
  if (fling.active && fling.free && !open) {
    return { x: fling.px - window.screenX - w / 2, y: fling.py - window.screenY - h / 2, w, h, style, dock, vertical };
  }
  // grabbed / flying: wherever the drag or the animation puts it
  if (fling.active && !open) {
    const local = fling.pos - (vertical ? window.screenY : window.screenX);
    if (vertical) y = local;
    else x = local;
  }
  return { x, y, w, h, style, dock, vertical };
}

// Both rectangles go to the backend, which runs the hover state machine (dwell, buttons, leave).
function reportRect() {
  if (MANAGER) return;
  const dpr = window.devicePixelRatio || 1;
  const phys = (r, pad) => [(r.x - pad) * dpr, (r.y - pad) * dpr, (r.w + 2 * pad) * dpr, (r.h + 2 * pad) * dpr];
  const c = islandRect(false);
  // The open panel's hover zone reaches the docked screen edge: the panel floats a few px off it,
  // and with the line (whose zone does touch the edge) the pointer pushed against the edge opened
  // the panel and was then outside it at once, so it closed straight away.
  const p = { ...islandRect(true) };
  const W = window.innerWidth, H = window.innerHeight;
  if (p.dock === "top") { p.h += p.y; p.y = 0; }
  else if (p.dock === "bottom") p.h = H - p.y;
  else if (p.dock === "left") { p.w += p.x; p.x = 0; }
  else p.w = W - p.x;
  // the thin line gets a bigger hover zone than it looks
  invoke("set_hit_rects", { pill: phys(c, c.style === "line" ? 10 : 4), panel: phys(p, 4) }).catch(() => {});
}

function applySize() {
  if (MANAGER) return;
  document.documentElement.style.setProperty("--pill-h", sz("pillH") + "px");
  const r = islandRect(expanded);
  const fixedW = !r.vertical && sz("pillW") > 0;
  pill.style.width = fixedW ? r.w + "px" : "";
  pill.style.justifyContent = fixedW ? "center" : "";
  island.classList.toggle("notch", r.style === "notch");
  const card = nst.on || (tour.on && !fling.active);
  island.classList.toggle("line", r.style === "line" && !expanded && !card);
  island.classList.toggle("vertical", r.vertical && !expanded && !card);
  for (const d of ["top", "bottom", "left", "right"]) island.classList.toggle("dock-" + d, r.dock === d);
  island.style.setProperty("--ix", r.x + "px");
  island.style.setProperty("--iy", r.y + "px");
  island.style.width = r.w + "px";
  island.style.height = r.h + "px";
  if (!expanded && card) island.style.borderRadius = r.style === "notch" ? "" : "22px";
  else if (!expanded && r.style === "capsule") island.style.borderRadius = r.h / 2 + "px";
  else island.style.borderRadius = "";
  placeTail(r);
  reportRect();
  if (editing) renderEditUi();
}

function expand() {
  if (expanded) return;
  hideNotes(); // they stay on the notes page
  // opened while a note shows in the pill: that's what the user wants to see
  if (nfl.until > Date.now()) {
    nfl.until = 0;
    if (visiblePages().includes("notes")) tab = "notes";
  }
  expanded = true;
  island.classList.remove("peek");
  island.classList.add("expanded");
  island.classList.remove("collapsed");
  $("panel").scrollTop = 0;
  if (tab === "sys") renderList(true); // fresh numbers instead of the ones from the last time
  renderPanel();
  applySize();
  ensureSysPoll();
  updateViz();
  Cascade.play($("list"));
  if (tour.on) {
    if (tour.step === 0) tour.step = 1;
    renderTour();
  }
  // the pill's network alert was clicked, or a problem not looked at yet: the diagnosis first
  const p = diag.problem;
  if (!tour.on && (diagWant || (p && sget("diag.on", true) && diagSeen !== p.downSince))) {
    diagWant = false;
    if (p) diagSeen = p.downSince;
    setTimeout(() => { if (expanded) openDiagSheet(true); }, 360);
  }
}
let diagSeen = null;

function collapse() {
  if (!expanded || MANAGER) return;
  expanded = false;
  closeMenu();
  confirmDisable = false;
  island.classList.remove("expanded");
  island.classList.add("collapsed");
  $("search").blur();
  Sheet.close();
  if (tour.on) renderTour();
  applySize();
  updateViz();
}

// ---------- pill ----------

// At most 4 characters ("0", "999B", "9.9K", "512K", "12M"…) so the pill keeps one width: each
// number sits in a fixed 4-character slot (see .sv), a jumping speed never resizes the island.
function shortRate(b) {
  if (!b || b < 1) return "0";
  if (b < 1000) return `${Math.round(b)}B`;
  const units = ["K", "M", "G"];
  let v = b / 1024;
  for (const u of units) {
    if (v < 9.95) return `${v.toFixed(1)}${u}`;
    if (v < 999.5) return `${Math.round(v)}${u}`;
    v /= 1024;
  }
  return "999G";
}

function clockText() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}${sget("pill.clockSeconds", false) ? ":" + p(d.getSeconds()) : ""}`;
}

// One pill item; "" when it has nothing to show.
function pillItem(id, target) {
  switch (id) {
    case "rotate":
      return rotHtml(target);
    case "exit":
      return `${target?.cc ? flagImg(target.cc) : ""}<span class="mono ip-text">${esc(target?.ip || (target?.error ? "出口检测失败" : "检测中…"))}</span>`;
    case "conns":
      return `<span class="mono pn pn3">${snap ? snap.connCount : 0}</span><span class="pill-unit">连接</span>`;
    case "speed":
      return `<span class="mono pill-speed"><span class="u">↑</span><span class="sv">${shortRate(snap?.up)}</span> <span class="d">↓</span><span class="sv">${shortRate(snap?.down)}</span></span>`;
    case "latency": {
      const L = snap?.latency;
      if (!L?.at) return "";
      if (L.ms == null) return `<span class="lat bad">节点${esc(L.error || "无响应")}</span>`;
      const c = L.ms < 200 ? "good" : L.ms < 500 ? "mid" : "bad";
      return `<span class="lat ${c} mono pn pn6">${Math.min(L.ms, 9999)}ms</span>`;
    }
    case "leaks": {
      const n = snap?.guard?.unacked || 0;
      return n ? `<span class="pill-alert">⚠ 泄漏 ${n}</span>` : "";
    }
    case "cpu":
      return snap?.sys?.cpu != null ? `<span class="pill-unit">CPU</span><span class="mono pn pn4">${Math.round(snap.sys.cpu)}%</span>` : "";
    case "mem":
      return snap?.sys?.memTotal ? `<span class="pill-unit">内存</span><span class="mono pn pn4">${Math.round((snap.sys.memUsed / snap.sys.memTotal) * 100)}%</span>` : "";
    case "clock":
      return `<span class="clock clock-${esc(sget("pill.clockStyle", "simple"))}">${clockText()}</span>`;
    case "notes": {
      if (!sget("notify.on", true)) return "";
      const un = notes.filter((n) => !n.read && !n.silent);
      if (!un.length) return "";
      if (noteStyle() === "icon") {
        // one small icon per app with unread notes (newest first, overlapping, at most 3)
        const apps = [];
        for (const n of un) if (!apps.some((a) => a.key === n.key)) apps.push(n);
        return `<span class="pill-nicons" title="${apps.map((a) => esc(a.app)).join("、")}">${apps.slice(0, 3).map((n) =>
          `<span class="pni${n.id === notePop ? " pop" : ""}">${noteMini(n, false)}</span>`).join("")}${apps.length > 3 ? `<span class="pni-more mono">+${apps.length - 3}</span>` : ""}</span>`;
      }
      const u = un.length;
      return `<span class="pill-bell"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="${BELL}"/></svg><span class="mono">${u > 99 ? "99+" : u}</span></span>`;
    }
    case "exits": {
      // every measured group's exit side by side (just the main exit without a proxy client)
      // one entry per exit IP; the tag names its main group (+N more) only when the IPs differ
      const exits = exitsByIp((exitState?.groups || []).filter((g) => g.ip));
      if (!snap?.proxy?.detected || !exits.length) return pillItem("exit", target);
      if (exits.length === 1) return `<span class="px-exit">${flagImg(exits[0].cc)}<span class="mono ip-text">${esc(exits[0].ip)}</span></span>`;
      // several exits: stacked on top of each other in small type (at most three lines; the rest
      // are counted, the panel lists them all) - group tag | flag | IP, IPs lined up
      const shown = exits.slice(0, 3);
      const more = exits.length - shown.length;
      return `<span class="px-stack n${shown.length}">${shown.map((x) =>
        `<span class="px-tag">${esc(exitTag(x))}</span>${flagImg(x.cc)}<span class="mono px-ip">${esc(x.ip)}</span>`).join("")}</span>` +
        (more ? `<span class="rot-tag">+${more}</span>` : "");
    }
    case "media": {
      if (!sget("media.on", true) || !media?.title || !media.playing) return "";
      const cover = media.cover && sget("media.cover", true) ? `<img class="rot-cover" src="${media.cover}" alt="">` : `<span class="rot-note">♪</span>`;
      // default: just the cover and bars moving with the music; or the title as before
      if (sget("media.pillStyle", "viz") === "viz") return `${cover}${vizBars(PILL_BANDS.length)}`;
      return `${cover}<span class="rot-text">${esc(media.title)}${media.artist ? `<span class="rot-sub"> · ${esc(media.artist)}</span>` : ""}</span>`;
    }
  }
  if (id.startsWith("ai-")) {
    if (!sget("ai.on", true)) return "";
    const q = aiState.find((x) => x.tool === id.slice(3));
    return q ? ringPill(q) : "";
  }
  return "";
}

// ---------- music bars (levels from the backend's loopback reader, 12 bands) ----------

const PILL_BANDS = [1, 3, 5, 7, 10]; // the pill's five bars: low to high
const vizBars = (n, big) => `<span class="mviz${big ? " big" : ""}" aria-hidden="true">${"<i></i>".repeat(n)}</span>`;
let vizOn = null;

// bars are wanted while a song plays and some bars are on screen (pill or music page)
function updateViz() {
  const playing = sget("media.on", true) && media?.playing;
  const onPill = !MANAGER && !expanded && !!pill.querySelector(".mviz");
  const onPage = tab === "music" && (MANAGER ? !document.hidden : expanded) && !!document.querySelector("#music .mviz");
  const want = !!(playing && (onPill || onPage));
  if (want === vizOn) return;
  vizOn = want;
  invoke("audio_viz", { on: want }).catch(() => {});
  if (!want) document.querySelectorAll(".mviz i").forEach((b) => (b.style.transform = ""));
}
listen("viz", (e) => {
  const b = e.payload || [];
  document.querySelectorAll(".mviz").forEach((el) => {
    const bars = el.children;
    const pick = el.classList.contains("big") ? null : PILL_BANDS;
    for (let i = 0; i < bars.length; i++) {
      const v = b[pick ? pick[i] : i] || 0;
      bars[i].style.transform = `scaleY(${(0.12 + 0.88 * v).toFixed(3)})`;
    }
  });
});
document.addEventListener("visibilitychange", () => updateViz());

// ---------- quota rings ----------

// Brand marks (lobe-icons, MIT): files in ui/brands; Grok is one path drawn in the text colour so it
// reads on both themes. A tool without a mark falls back to its initial.
const BRAND_FILE = { claude: "brands/claude.svg", codex: "brands/codex.svg", antigravity: "brands/antigravity.svg" };
const GROK_PATH = "M9.27 15.29l7.978-5.897c.391-.29.95-.177 1.137.272.98 2.369.542 5.215-1.41 7.169-1.951 1.954-4.667 2.382-7.149 1.406l-2.711 1.257c3.889 2.661 8.611 2.003 11.562-.953 2.341-2.344 3.066-5.539 2.388-8.42l.006.007c-.983-4.232.242-5.924 2.75-9.383.06-.082.12-.164.179-.248l-3.301 3.305v-.01L9.267 15.292M7.623 16.723c-2.792-2.67-2.31-6.801.071-9.184 1.761-1.763 4.647-2.483 7.166-1.425l2.705-1.25a7.808 7.808 0 00-1.829-1A8.975 8.975 0 005.984 5.83c-2.533 2.536-3.33 6.436-1.962 9.764 1.022 2.487-.653 4.246-2.34 6.022-.599.63-1.199 1.259-1.682 1.925l7.62-6.815";
const hasBrand = (tool) => !!BRAND_FILE[tool] || tool === "grok";
// as an HTML element
function brandIcon(tool, px, cls = "") {
  if (BRAND_FILE[tool]) return `<img class="brand-ic ${cls}" src="${BRAND_FILE[tool]}" width="${px}" height="${px}" alt="">`;
  if (tool === "grok") return `<svg class="brand-ic grok ${cls}" width="${px}" height="${px}" viewBox="0 0 24 24" aria-hidden="true"><path d="${GROK_PATH}"/></svg>`;
  return "";
}
// inside an SVG, centred on (c, c), s wide
function brandInSvg(tool, c, s) {
  const x = c - s / 2;
  if (BRAND_FILE[tool]) return `<image href="${BRAND_FILE[tool]}" x="${x}" y="${x}" width="${s}" height="${s}"/>`;
  return `<svg class="grok" x="${x}" y="${x}" width="${s}" height="${s}" viewBox="0 0 24 24"><path d="${GROK_PATH}"/></svg>`;
}
const ringLevel = (used) => (used >= Number(sget("ai.ring.crit", 85)) ? "crit" : used >= Number(sget("ai.ring.warn", 60)) ? "warn" : "ok");

// The windows a tool's ring shows: [outer, inner?] from its setting (both | first | second | max).
function ringWindows(q) {
  const ws = (q.windows || []).filter((w) => w && w.used != null);
  if (!ws.length) return [];
  const mode = sget(`ai.ring.mode.${q.tool}`, "both");
  if (mode === "max") return [ws.reduce((a, b) => (b.used > a.used ? b : a))];
  // several pools (Antigravity: Gemini, Claude / GPT, each with 5 h + weekly): the busier pool
  if (ws.some((w) => w.group)) {
    const busiest = ws.reduce((a, b) => (b.used > a.used ? b : a)).group;
    const pool = ws.filter((w) => w.group === busiest);
    if (mode === "first") return [pool[0]];
    if (mode === "second") return [pool[1] || pool[0]];
    return pool.slice(0, 2);
  }
  if (mode === "first") return [ws[0]];
  if (mode === "second") return [ws[1] || ws[0]];
  if (mode === "max") return [ws.reduce((a, b) => (b.used > a.used ? b : a))];
  return ws.slice(0, 2);
}

// SVG ring: one arc per window (outer = first), the tool's mark (or given text) in the middle.
function ringSvg(q, size, stroke, center) {
  const ws = ringWindows(q);
  const c = size / 2;
  // the hole inside the innermost arc
  const hole = 2 * (c - stroke - Math.max(0, ws.length - 1) * (stroke + 1.5));
  const mid = center != null || !hasBrand(q.tool)
    ? `<text x="${c}" y="${c}" dy="0.36em" text-anchor="middle">${center ?? esc((q.name || "?")[0])}</text>`
    : brandInSvg(q.tool, c, Math.max(8, Math.min(hole - 1, size * 0.62)));
  const arcs = ws.map((w, i) => {
    const r = c - stroke / 2 - i * (stroke + 1.5);
    const len = 2 * Math.PI * r;
    const pct = Math.max(0, Math.min(100, w.used));
    return `<circle class="rg-track" cx="${c}" cy="${c}" r="${r}" stroke-width="${stroke}"/>` +
      (pct > 0 ? `<circle class="rg-arc ${ringLevel(pct)}" cx="${c}" cy="${c}" r="${r}" stroke-width="${stroke}" stroke-dasharray="${(len * pct) / 100} ${len}" transform="rotate(-90 ${c} ${c})"/>` : "");
  }).join("");
  return `<svg class="ring" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}" aria-hidden="true">${arcs}${mid}</svg>`;
}

// pill item: ring + the share used (both windows: "44/12%")
function ringPill(q) {
  if (!q.windows?.length || (q.status !== "ok" && q.status !== "stale")) return "";
  const ws = ringWindows(q);
  if (!ws.length) return "";
  const size = Math.max(20, Math.round(sz("pillH") * 0.6));
  const nums = ws.map((w, i) => (i ? `<span class="rg-sub">/${Math.round(w.used)}</span>` : Math.round(w.used))).join("");
  return `<span class="ring-item ${q.status}">${ringSvg(q, size, ws.length > 1 ? 2.5 : 3)}<span class="mono">${nums}%</span></span>`;
}

// ---------- rotating slot (exits per group, now playing, AI quota, speed, latency) ----------

let aiState = [];
let media = null;
const rot = { key: null, at: 0, anim: false };

// "🚀 代理设置" -> "代理设置"; long names cut to 8 characters
function shortGroup(name, isDefault) {
  const s = String(name || "").replace(/[\p{Extended_Pictographic}\u{1F1E6}-\u{1F1FF}️‍]/gu, "").trim();
  if (!s) return isDefault ? "代理" : "分组";
  return [...s].length > 8 ? [...s].slice(0, 8).join("") + "…" : s;
}

function quotaClass(used) {
  const l = ringLevel(used);
  return l === "ok" ? "" : l;
}

function shortWindow(label) {
  if (label === "5 小时") return "5h";
  if (label === "每周") return "周";
  if (label === "每月") return "月";
  return [...label].length > 8 ? [...label].slice(0, 8).join("") + "…" : label;
}

function rotCards(target) {
  const cards = [];
  const items = sget("rotate.items", null) || [{ id: "exits", on: true }, { id: "media", on: true }, { id: "ai", on: true }];
  const detected = snap?.proxy?.detected;
  for (const it of items) {
    if (!it.on) continue;
    if (it.id === "exits") {
      const exits = exitsByIp((exitState?.groups || []).filter((g) => g.ip));
      if (detected && exits.length) {
        for (const x of exits) {
          cards.push({ key: "x:" + x.ip, html: `${exits.length > 1 ? `<span class="rot-tag">${esc(exitTag(x))}</span>` : ""}${flagImg(x.cc)}<span class="mono ip-text">${esc(x.ip)}</span>` });
        }
      } else {
        cards.push({ key: "x:main", html: pillItem("exit", target) });
      }
    } else if (it.id === "media") {
      if (media?.title && media.playing) {
        const cover = media.cover && sget("media.cover", true) ? `<img class="rot-cover" src="${media.cover}" alt="">` : `<span class="rot-note">♪</span>`;
        cards.push({ key: "m:" + media.title, html: `${cover}<span class="rot-text">${esc(media.title)}${media.artist ? `<span class="rot-sub"> · ${esc(media.artist)}</span>` : ""}</span>` });
      }
    } else if (it.id === "ai") {
      for (const q of aiState) {
        const h = ringPill(q);
        if (h) cards.push({ key: "a:" + q.tool, html: h });
      }
    } else if (it.id === "speed" || it.id === "latency") {
      const h = pillItem(it.id, target);
      if (h) cards.push({ key: "s:" + it.id, html: h });
    }
  }
  return cards;
}

function rotHtml(target) {
  return rotateCards(rotCards(target), "main");
}

// What a pill item id or a rotation card key shows, so the rotation can skip what already stands
// in the pill: "exits" / "media" / "speed" / "latency" / "ai:<tool>".
function coverOf(k) {
  if (k === "exit" || k === "exits" || k.startsWith("x:")) return "exits";
  if (k === "media" || k.startsWith("m:")) return "media";
  if (k.startsWith("ai-")) return "ai:" + k.slice(3);
  if (k.startsWith("a:")) return "ai:" + k.slice(2);
  if (k.startsWith("s:")) return k.slice(2);
  return k;
}

// A slot that shows one card at a time, advancing every rotate.interval seconds.
const rots = { main: rot };
function rotateCards(cards, name) {
  if (!cards.length) return "";
  const r = rots[name] || (rots[name] = { key: null, at: 0, anim: false });
  const every = Math.max(2, Number(sget("rotate.interval", 5)) || 5) * 1000;
  let i = cards.findIndex((c) => c.key === r.key);
  const now = Date.now();
  if (i < 0) {
    i = 0;
    r.at = now;
  } else if (now - r.at >= every && cards.length > 1) {
    i = (i + 1) % cards.length;
    r.at = now;
  }
  if (cards[i].key !== r.key) {
    r.anim = r.key !== null;
    r.key = cards[i].key;
  }
  const html = `<span class="rot-card${r.anim ? " rot-in" : ""}">${cards[i].html}</span>`;
  r.anim = false;
  return html;
}

// ---------- live activities (front of the pill): AI sessions at work, network trouble ----------

let agents = [];
const diag = { last: null, problem: null, running: false, prog: null };
let diagSheetOpen = false;
let diagWant = false; // open the diagnosis sheet once the island has opened

function elapsed(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  const p = (n) => String(n).padStart(2, "0");
  return h ? `${h}:${p(m)}:${p(x)}` : `${m}:${p(x)}`;
}
const durText = (ms) => {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分 ${s % 60} 秒`;
  return `${Math.floor(s / 3600)} 小时 ${Math.floor((s % 3600) / 60)} 分`;
};

// These items are rebuilt every second (their timers tick): looping animations get a negative
// delay from the clock so they carry on instead of restarting, and the entrance plays only
// during the first half second an item exists.
const livePhase = () => {
  const t = Date.now();
  return `--ph9:-${t % 900}ms;--ph11:-${t % 1100}ms;--ph14:-${t % 1400}ms;--ph16:-${t % 1600}ms;--ph18:-${t % 1800}ms`;
};
const liveBorn = new Map();
function liveEnter(key) {
  const now = Date.now();
  if (!liveBorn.has(key)) liveBorn.set(key, now);
  return now - liveBorn.get(key) < 500 ? " lv-enter" : "";
}
function liveForget(keys) {
  for (const k of liveBorn.keys()) if (!keys.includes(k)) liveBorn.delete(k);
}

// the session to jump to: one waiting for the user first, then the longest running
const agentLead = () => agents.find((a) => a.state === "ask") || [...agents].sort((a, b) => a.started - b.started)[0];

function agentsPill() {
  if (!agents.length || !sget("ai.live", true)) return "";
  const ask = agents.filter((a) => a.state === "ask").length;
  const lead = agentLead();
  const st = ask ? "ask" : "run";
  const tip = agents.map((a) => `${a.project}：${a.state === "ask" ? a.ask || "等你确认" : "运行 " + elapsed(Date.now() - a.started)}`).join("\n") + "\n点一下切到它的窗口";
  const name = agents.length === 1 ? esc(lead.project) : `${agents.length} 个任务`;
  const tail = ask
    ? `<span class="ag-ask">${agents.length === 1 ? "等你确认" : `${ask} 个等你`}</span>`
    : `<span class="ag-t mono">${elapsed(Date.now() - lead.started)}</span>`;
  return `<span class="ag-pill ${st}${liveEnter("pill-ag")}" style="${livePhase()}" data-ag="${esc(lead.id)}" title="${esc(tip)}"><span class="ag-ic ${st}">${brandIcon(lead.tool, 15)}<i class="ag-orbit"></i></span><span class="ag-name">${name}</span>${tail}</span>`;
}

const DG_PATH = {
  wifiOff: "M2 2l20 20M8.5 16.4a5 5 0 0 1 7 0M5 12.9a10 10 0 0 1 5.2-2.7M19 12.9a10 10 0 0 0-1.9-1.5M2 8.8a15 15 0 0 1 4.3-2.6M22 8.8a15 15 0 0 0-11-3.7M12 20h.01",
  plug: "M9 2v5M15 2v5M6 7h12v4a6 6 0 0 1-12 0zM12 17v5",
  pulse: "M3 12h4l3-8 4 16 3-8h4",
};

function diagPill() {
  const p = diag.problem;
  if (!p || !sget("diag.on", true)) return "";
  const sys = p.kind === "sysproxy";
  const since = !sys && p.downSince ? `<span class="dg-t mono">${elapsed(Date.now() - p.downSince)}</span>` : "";
  return `<span class="dg-pill ${sys ? "warn" : "bad"}${liveEnter("pill-dg")}" style="${livePhase()}" data-dg="1" title="${esc(p.verdict || "")}\n点一下看诊断">
    <span class="dg-ic"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="${sys ? DG_PATH.plug : DG_PATH.wifiOff}"/></svg></span>
    <span class="dg-txt">${esc(p.problem || "网络异常")}</span>${since}</span>`;
}

// a short press on a live item: jump to the session's window / open the diagnosis
function liveClick(t) {
  const ag = t?.closest?.("[data-ag]");
  if (ag) {
    invoke("agents_focus", { id: ag.dataset.ag }).catch((e) => fireFlash("agent", String(e), "warn"));
    return true;
  }
  if (t?.closest?.("[data-dg]")) {
    diagWant = true;
    invoke("pill_click").catch(() => {});
    return true;
  }
  return false;
}

listen("agents", (e) => {
  agents = e.payload || [];
  renderPill();
  if (expanded) renderLiveBar();
  if (expanded && tab === "ai") renderList();
});
invoke("agents_get").then((l) => { agents = l || []; renderPill(); }).catch(() => {});

// ----- network diagnosis -----

const DG_STEPS = [["nic", "网卡"], ["gw", "路由器"], ["dns", "DNS"], ["net", "互联网"], ["core", "代理软件"], ["node", "节点"]];
const DG_ICON = {
  nic: "M3 5h18v11H3zM8 20h8M12 16v4",
  gw: "M3 14h18v6H3zM7 17h.01M11 17h.01M8.5 10a5 5 0 0 1 7 0M6 7.5a8.5 8.5 0 0 1 12 0",
  dns: "M12 3v18M5 5h11l2.5 2.5L16 10H5zM19 13H8l-2.5 2.5L8 18h11z",
  net: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c2.4 2.5 3.6 5.5 3.6 9s-1.2 6.5-3.6 9c-2.4-2.5-3.6-5.5-3.6-9S9.6 5.5 12 3z",
  core: "M12 3l8 3v6c0 4.6-3.3 7.9-8 9-4.7-1.1-8-4.4-8-9V6z",
  node: "M7 18h10a4 4 0 0 0 .6-7.95A6 6 0 0 0 6.1 11.6 3.3 3.3 0 0 0 7 18z",
};
const DG_BADGE = { ok: "M6 12.5l4 4 8-9", fail: "M7 7l10 10M17 7L7 17", warn: "M12 7v6M12 17h.01" };
const svgI = (d, w = 18, sw = 1.9) => `<svg width="${w}" height="${w}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>`;

function dgLink(a, b) {
  if (b.status === "run") return "flow";
  if (a.status === "fail" || b.status === "fail") return "broken";
  if (b.status === "skip" || b.status === "wait") return "idle";
  if (a.status === "warn" || b.status === "warn") return "warn";
  return "ok";
}

// what the sheet shows: the check in progress, a system proxy problem, or the last check
function dgShown() {
  if (diag.running && diag.prog) return diag.prog;
  if (diag.problem?.kind === "sysproxy") return diag.problem;
  return diag.last || diag.problem;
}

function diagSheetHtml() {
  const r = dgShown();
  const running = diag.running;
  const steps = r?.steps?.length ? r.steps : r?.kind === "sysproxy" ? [] : DG_STEPS.map(([id, label]) => ({ id, label, status: "wait", detail: "" }));
  const chain = steps.length ? `<div class="dg-chain">${steps.map((s, i) => {
    const link = i ? `<span class="dg-link ${dgLink(steps[i - 1], s)}"><i></i></span>` : "";
    const badge = DG_BADGE[s.status] ? `<b class="dg-badge">${svgI(DG_BADGE[s.status], 10, 3.2)}</b>` : "";
    const det = s.detail || (s.status === "run" ? "检测中…" : s.status === "skip" ? "跳过" : "");
    return `${link}<div class="dg-node ${s.status}" style="--i:${i}">
      <span class="dg-orb">${svgI(DG_ICON[s.id] || DG_ICON.net)}${badge}</span>
      <span class="dg-lab">${esc(s.label)}</span>
      <span class="dg-det" title="${esc(det)}">${esc(det)}${s.ms != null && s.status === "ok" ? `<i class="mono">${s.ms} ms</i>` : ""}</span>
    </div>`;
  }).join("")}</div>` : "";
  let verdict;
  if (running) {
    verdict = `<div class="dg-verdict run"><span class="dg-big">${svgI(DG_PATH.pulse, 22, 2.2)}</span><div><b>正在逐项检查…</b><span>从网卡一路查到节点，十秒内出结果</span></div></div>`;
  } else if (!r) {
    verdict = `<div class="dg-verdict"><span class="dg-big">${svgI(DG_PATH.pulse, 22, 2.2)}</span><div><b>还没检查过</b><span>点下面的按钮开始</span></div></div>`;
  } else if (r.ok) {
    const n = (r.steps || []).filter((s) => s.status === "ok").length;
    verdict = `<div class="dg-verdict ok"><span class="dg-big">${svgI(DG_BADGE.ok, 22, 2.6)}</span><div><b>${esc(r.verdict || "网络正常")}</b><span>${n ? `${n} 项检查都通过了` : ""}${r.tookMs ? ` · 用时 ${(r.tookMs / 1000).toFixed(1)} 秒` : ""}</span></div></div>`;
  } else {
    const since = r.downSince ? `<span class="dg-since">已持续 ${durText(Date.now() - r.downSince)}</span>` : "";
    verdict = `<div class="dg-verdict ${r.kind === "sysproxy" ? "warn" : "bad"}"><span class="dg-big">${svgI(r.kind === "sysproxy" ? DG_PATH.plug : DG_BADGE.fail, 22, 2.4)}</span>
      <div><b>${esc(r.verdict)}</b>${since}<span>${esc(r.advice || "")}</span></div></div>
      ${r.fixes?.length ? `<div class="dg-fixes">${r.fixes.map((f) => `<button class="${f.primary ? "primary" : "ghost"}" data-dg-fix="${esc(f.id)}">${esc(f.label)}</button>`).join("")}</div>` : ""}`;
  }
  const when = r && !running ? `${r.auto ? "自动发现" : "手动检测"} · ${ago(r.at)}` : running ? "检测中" : "";
  return `<div class="as-head dg-head" data-sheet-drag><span class="dg-logo">${svgI(DG_PATH.pulse, 18, 2.2)}</span><div class="as-title"><b>网络诊断</b><span>${esc(when)}</span></div>
      <button class="ghost" data-sheet-do="close" title="关闭（Esc）">✕</button></div>
    ${chain}${verdict}
    <div class="m-actions end dg-foot"><button class="ghost" data-dg-fix="retry" ${running ? "disabled" : ""}>${running ? "检测中…" : "重新检测"}</button></div>`;
}

function diagSync() {
  if (diagSheetOpen && Sheet.isOpen()) Sheet.update(diagSheetHtml());
}

function startDiag() {
  if (diag.running) return;
  diag.running = true;
  diag.prog = { steps: DG_STEPS.map(([id, label]) => ({ id, label, status: "wait", detail: "" })) };
  diagSync();
  invoke("netdiag_run").then((r) => {
    diag.running = false;
    diag.prog = null;
    diag.last = r;
    diagSync();
  }).catch((e) => { diag.running = false; diag.prog = null; diagSync(); toast(String(e), true); });
}

function openDiagSheet(run) {
  if (MANAGER) return;
  sheetApp = null;
  diagSheetOpen = true;
  Sheet.open($("panel"), diagSheetHtml(), {
    fit: true,
    onClose: () => { diagSheetOpen = false; measurePanel(); },
    onClick: (e) => {
      if (e.target.closest("[data-sheet-do='close']")) { Sheet.close(); return; }
      const b = e.target.closest("[data-dg-fix]");
      if (!b || b.disabled) return;
      const id = b.dataset.dgFix;
      if (id === "retry") { startDiag(); return; }
      b.disabled = true;
      invoke("netdiag_fix", { id }).then((msg) => {
        if (msg) toast(msg);
        // check again once the fix had time to work
        const wait = { flushdns: 600, "switch-node": 1500, "restart-proxy": 5000, "start-proxy": 5000 }[id];
        if (wait) setTimeout(startDiag, wait);
        else diagSync();
      }).catch((err) => { b.disabled = false; toast(String(err), true); });
    },
  });
  measurePanel();
  // a fresh check unless one just ran (or is running)
  const fresh = diag.last && Date.now() - diag.last.at < 15_000;
  if (run && !diag.running && !fresh && diag.problem?.kind !== "sysproxy") startDiag();
}

listen("netdiag", (e) => {
  const s = e.payload || {};
  const was = diag.problem;
  diag.last = s.last || diag.last;
  diag.problem = s.problem || null;
  if (!s.running) { diag.running = false; diag.prog = null; } else diag.running = true;
  if (was?.kind === "net" && !diag.problem && diag.last?.recoveredMs) {
    fireFlash("recover", `网络恢复了 · 断了 ${durText(diag.last.recoveredMs)}`, "info");
  } else if (!was && diag.problem) {
    invoke("log_js", { msg: `netdiag problem: ${diag.problem.problem}` }).catch(() => {});
  }
  diagSync();
  renderPill();
  if (expanded) renderLiveBar();
  if (expanded && tab === "conns") renderList();
});
listen("netdiag-progress", (e) => {
  diag.running = true;
  diag.prog = e.payload;
  diagSync();
});
invoke("netdiag_get").then((s) => { diag.last = s?.last || null; diag.problem = s?.problem || null; diag.running = !!s?.running; renderPill(); }).catch(() => {});

// chips in the open island's top bar (clickable in every interaction mode)
let liveBarLast = "";
function renderLiveBar() {
  if (MANAGER) return;
  const el = $("pg-live");
  if (!el) return;
  const chips = [];
  if (diag.problem && sget("diag.on", true)) {
    const sys = diag.problem.kind === "sysproxy";
    chips.push(`<button class="lv-chip dg ${sys ? "warn" : "bad"}${liveEnter("bar-dg")}" style="${livePhase()}" data-do="diag-open" title="${esc(diag.problem.verdict || "")}">${svgI(sys ? DG_PATH.plug : DG_PATH.wifiOff, 13, 2.2)}<span>${esc(diag.problem.problem || "网络异常")}</span></button>`);
  }
  if (sget("ai.live", true)) {
    const list = [...agents].sort((a, b) => (a.state === "ask") === (b.state === "ask") ? a.started - b.started : a.state === "ask" ? -1 : 1);
    const shown = list.slice(0, chips.length ? 1 : 2);
    for (const a of shown) {
      chips.push(`<button class="lv-chip ag ${a.state}${liveEnter("bar-" + a.id)}" style="${livePhase()}" data-do="agent-focus" data-id="${esc(a.id)}" title="${esc(a.project)}：${a.state === "ask" ? esc(a.ask || "等你确认") : "正在工作"}\n点一下切到它的窗口">
        <span class="ag-ic ${a.state}">${brandIcon(a.tool, 13)}<i class="ag-orbit"></i></span><span class="lv-name">${esc(a.project)}</span>${a.state === "ask" ? `<span class="ag-ask">等你</span>` : `<span class="mono lv-t">${elapsed(Date.now() - a.started)}</span>`}</button>`);
    }
    const more = agents.length - shown.length;
    if (more > 0) chips.push(`<button class="lv-chip more" data-do="page-ai">+${more}</button>`);
  }
  const keep = [...liveBorn.keys()].filter((k) => k.startsWith("pill-") || (k === "bar-dg" && diag.problem) || agents.some((a) => k === "bar-" + a.id));
  liveForget(keep);
  const html = chips.join("");
  if (html.replace(/style="[^"]*"/g, "") !== liveBarLast.replace(/style="[^"]*"/g, "")) {
    liveBarLast = html;
    el.innerHTML = html;
  }
}

function agentsSectionHtml() {
  if (!agents.length || !sget("ai.live", true)) return "";
  const rows = [...agents].sort((a, b) => (a.state === "ask") === (b.state === "ask") ? a.started - b.started : a.state === "ask" ? -1 : 1).map((a) => `
    <div class="ag-row ${a.state}" style="${livePhase()}">
      <span class="ag-ic big ${a.state}">${brandIcon(a.tool, 20)}<i class="ag-orbit"></i></span>
      <div class="ag-main"><b>${esc(a.project)}</b><span>${a.state === "ask" ? esc(a.ask || "等你确认") : "正在工作"}</span></div>
      <span class="ag-time mono">${elapsed(Date.now() - a.started)}</span>
      <button class="${a.state === "ask" ? "primary" : "ghost"}" data-do="agent-focus" data-id="${esc(a.id)}">${a.state === "ask" ? "去确认" : "切过去"}</button>
    </div>`).join("");
  return `<div class="ag-list"><div class="ag-h">进行中<span>${agents.length}</span></div>${rows}</div>`;
}

// ---------- threshold flashes (5 s message in the pill) ----------

const flash = { msg: null, until: 0, cool: {}, prevExit: null, latAt: 0, latOk: true, latMs: null, hist: [] };

function fireFlash(kind, text, level) {
  const now = Date.now();
  if ((flash.cool[kind] || 0) > now) return;
  flash.cool[kind] = now + 60_000;
  flash.msg = { text, level };
  flash.until = now + 5000;
  invoke("log_js", { msg: `flash ${kind}: ${text}` }).catch(() => {});
  setTimeout(renderPill, 5100);
}

function checkFlashes(s) {
  if (!S) return;
  const proxyOn = s.proxy?.detected;
  const ip = (proxyOn ? s.exit?.proxy?.ip : s.exit?.direct?.ip) || null;
  if (sget("flash.exitChange", true) && ip && flash.prevExit && ip !== flash.prevExit) {
    fireFlash("exit", `出口 IP 变了：${flash.prevExit} → ${ip}`, "warn");
  }
  if (ip) flash.prevExit = ip;

  const L = s.latency;
  if (L?.at && L.at !== flash.latAt) {
    flash.latAt = L.at;
    const limit = sget("flash.latencyMs", 800);
    if (L.ms == null) {
      if (sget("flash.nodeDown", true) && flash.latOk) fireFlash("down", `节点无响应：${L.node || ""}`, "bad");
    } else if (sget("flash.latencyHigh", true) && L.ms > limit && !(flash.latMs > limit)) {
      fireFlash("lat", `节点延迟 ${L.ms} ms：${L.node || ""}`, "warn");
    }
    flash.latOk = L.ms != null;
    flash.latMs = L.ms;
  }

  const tot = (s.up || 0) + (s.down || 0);
  flash.hist.push(tot);
  if (flash.hist.length > 12) flash.hist.shift();
  const limitB = sget("flash.speedMBps", 5) * 1024 * 1024;
  const before = flash.hist.slice(0, -2);
  const avg = before.length ? before.reduce((a, b) => a + b, 0) / before.length : tot;
  if (sget("flash.speedSpike", true) && before.length >= 6 && tot > limitB && avg < limitB / 3) {
    const top = (s.apps || []).filter((a) => !a.isProxy).sort((a, b) => b.up + b.down - (a.up + a.down))[0];
    fireFlash("speed", `网速突增 ↓${shortRate(s.down)}/s ↑${shortRate(s.up)}/s${top ? " · " + top.name : ""}`, "info");
  }
}

let pillLast = "";
function renderPill() {
  if (MANAGER) return;
  const p = snap?.proxy;
  const ex = exitState;
  const target = mainExit(ex, p?.detected);
  let dot = "dot";
  if (ex?.checking) dot += " busy";
  if (target?.ip) dot += p?.detected ? " ok" : " warn";
  else if (target?.error) dot += " err";
  if (snap?.guard?.unacked) dot = "dot err busy";
  const netDown = diag.problem?.kind === "net" && sget("diag.on", true);
  if (netDown) dot = "dot err busy";
  // the thin-line style shows the same status as a colour
  const st = netDown || snap?.guard?.unacked || (!target?.ip && target?.error) ? "err" : target?.ip ? (p?.detected ? "ok" : "warn") : "idle";
  for (const s of ["ok", "warn", "err", "idle"]) island.classList.toggle("st-" + s, s === st);

  let html;
  if (flash.msg && Date.now() < flash.until) {
    html = `<span class="flash-msg ${flash.msg.level}">${decorate(flash.msg.text)}</span>`;
  } else if (nfl.until > Date.now() && noteById(nfl.id) && !shape().vertical && shape().style !== "line") {
    html = inlineNoteHtml();
  } else {
    flash.msg = null;
    const list = (sget("pill.items", null) || [{ id: "exits", on: true }, { id: "speed", on: true }, { id: "leaks", on: true }]).filter((i) => i.on);
    // the rotating slot skips whatever already stands in the pill on its own (no exit IP or quota
    // ring twice); with nothing left it disappears
    const shown = new Set(list.filter((i) => i.id !== "rotate").map((i) => coverOf(i.id)));
    const items = list
      .map((i) => {
        if (i.id !== "rotate") return { id: i.id, html: pillItem(i.id, target) };
        const cards = rotCards(target).filter((c) => !shown.has(coverOf(c.key)));
        return { id: "rotate", cards, html: rotateCards(cards, "main") };
      })
      .filter((i) => i.html);
    // live activities lead the pill (they never fold away: folding starts from the end)
    const live = [{ id: "diag", html: diagPill() }, { id: "agents", html: agentsPill() }].filter((i) => i.html);
    liveForget([...live.map((i) => "pill-" + (i.id === "diag" ? "dg" : "ag")), ...[...liveBorn.keys()].filter((k) => !k.startsWith("pill-"))]);
    html = foldPill(dot, [...live, ...items]);
  }
  const full = `<span class="${dot}"></span>${html}`;
  if (full !== pillLast) {
    pillLast = full;
    pill.innerHTML = full;
  }
  if (!expanded) applySize();
  updateViz();
}

const pillSpan = (it) => `<span class="pill-item${it.html.startsWith('<span class="rot-card') ? " rot-slot" : ""}" data-pi="${esc(it.id)}">${it.html}</span>`;
const PILL_SEP = `<span class="pill-sep"></span>`;
const fold = { sig: "", keep: 0, slotW: 0 };

// Items sit side by side in their order; the ones at the end that don't fit the width take turns
// in one rotating slot (as wide as the widest of them, so the pill doesn't jump).
function foldPill(dot, items) {
  const all = items.map(pillSpan).join(PILL_SEP);
  if (shape().vertical || items.length < 2) return all;
  const fixed = sz("pillW");
  const limit = fixed > 0 ? fixed : sz("pillMaxW");
  // measure only when the set of items (or the width) changed noticeably; text changes inside an
  // item (speeds, percentages) reuse the last decision
  const sig = `${limit}|${items.map((i) => i.id).join(",")}|${Math.round(all.length / 40)}`;
  if (sig !== fold.sig) {
    fold.sig = sig;
    pill.style.width = "";
    pill.innerHTML = `<span class="${dot}"></span>${all}`;
    pillLast = "";
    const els = [...pill.querySelectorAll(":scope > .pill-item")];
    const widths = els.map((e) => e.getBoundingClientRect().width);
    const gap = 8, sepW = 1 + 2 * gap;
    let total = pill.scrollWidth;
    let keep = items.length;
    let slotW = 0;
    if (total > limit) {
      // fold from the end until the kept items plus the rotating slot fit
      const base = total - widths.reduce((a, b) => a + b, 0) - (items.length - 1) * sepW;
      while (keep > 1) {
        keep--;
        slotW = Math.max(...widths.slice(keep));
        const w = base + widths.slice(0, keep).reduce((a, b) => a + b, 0) + keep * sepW + slotW;
        if (w <= limit) break;
      }
      // a kept rotation slot moves into the folded slot (one rotating slot, never two)
      const ri = items.findIndex((i) => i.id === "rotate");
      if (ri >= 0 && ri < keep) slotW = Math.max(slotW, widths[ri]);
      const keptW = widths.slice(0, keep).reduce((a, b) => a + b, 0) - (ri >= 0 && ri < keep ? widths[ri] + sepW : 0);
      // the slot shrinks to what is left when even one kept item plus the widest card is too wide
      const left = limit - (base + keptW + keep * sepW);
      slotW = Math.max(60, Math.min(slotW, left));
    }
    fold.keep = keep;
    fold.slotW = slotW;
  }
  if (fold.keep >= items.length) return all;
  const keptItems = items.slice(0, fold.keep).filter((i) => i.id !== "rotate");
  const tail = [...items.slice(fold.keep), ...items.slice(0, fold.keep).filter((i) => i.id === "rotate")];
  // folded items take turns with the rotation's own cards in the same slot (no slot inside a slot)
  const cards = tail.flatMap((i) => (i.id === "rotate" ? i.cards : [{ key: "f:" + i.id, html: i.html }]));
  if (!cards.length) return keptItems.map(pillSpan).join(PILL_SEP);
  const kept = keptItems.map(pillSpan).join(PILL_SEP);
  return `${kept}${kept ? PILL_SEP : ""}<span class="pill-item rot-slot fold" style="width:${Math.ceil(fold.slotW)}px">${rotateCards(cards, "fold")}</span>`;
}
// the rotating slot advances on its own, between snapshots too
if (!MANAGER) setInterval(() => { if (!expanded) renderPill(); }, 1000);

// ---------- notifications ----------
// A note springs out of the island like on an iPhone: the pill becomes a card (app icon or the
// sender's picture, app · sender, two lines, "刚刚"); several stack up (the edges of the ones
// behind peek out under the island), the cursor on the stack fans it out into a list and holds it.
// The notes page keeps today's notes grouped by app (in memory only, like the backend).

let notes = []; // newest first
const NOTE_W = 400;
const nst = { active: [], hover: false, fan: false, timer: 0, h: 0, on: false };
let gameOn = false;
const BELL = "M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0";
// a note without an app icon: the AI tool's mark, else the app's initial
const NOTE_BRAND = { claude: "brands/claude-tile.svg", codex: "brands/codex.svg" };
function noteGlyph(n) {
  if (NOTE_BRAND[n.key]) return `<img class="app-icon n-brand b-${n.key}" src="${NOTE_BRAND[n.key]}" alt="">`;
  return `<div class="app-icon letter n-glyph g-${esc(n.key.replace(/[^a-z0-9-]/gi, ""))}">${esc([...(n.app || "?")][0])}</div>`;
}

const noteById = (id) => notes.find((n) => n.id === id);
const unreadCount = () => notes.filter((n) => !n.read).length;

// privacy: 0 = everything, 1 = no message text (default "source": app + sender), 2 = only the
// app ("smart" while a fullscreen app is in front or a screen recorder runs)
function noteHidden(n) {
  if (n.source === "micabar") return 0; // MicaBar's own notes carry no private text
  const p = sget("notify.privacy", "source");
  if (p === "full") return 0;
  if (p === "smart") return gameOn || n.capturing ? 2 : 0;
  return 1;
}

function noteIcon(n) {
  const app = n.iconUrl
    ? `<img class="n-appimg" src="${n.iconUrl}" alt="">`
    : n.icon
      ? iconHtml({ path: n.icon, name: n.app })
      : noteGlyph(n);
  if (n.avatar && noteHidden(n) < 2) return `<div class="n-ico avatar"><img class="n-av" src="${n.avatar}" alt=""><span class="n-badge">${app}</span></div>`;
  if (n.kind === "firstnet") return `<div class="n-ico">${n.icon ? iconHtml({ path: n.icon, name: n.title }) : app}<span class="n-fn">${svgI(DG_ICON.net, 11, 2.4)}</span></div>`;
  return `<div class="n-ico">${app}</div>`;
}

function noteText(n) {
  const h = noteHidden(n);
  if (!h) return { title: n.title, body: n.body, sub: n.sub };
  // without the text: a neutral line (AI notes say what happened, not what they wrote)
  const body = n.kind === "done" ? n.sub || "任务完成" : n.kind === "ask" ? "等你确认" : n.count > 1 ? `${n.count} 条新消息` : n.source === "flash" && n.app !== "微信" ? "需要你注意" : "新消息";
  return { title: h === 2 ? n.app : n.title, body, sub: "" };
}

function noteCardHtml(n, close) {
  const t = noteText(n);
  const showApp = t.title !== n.app;
  const cnt = n.count > 1 && n.source !== "flash" && noteHidden(n) < 2 ? `<span class="n-cnt">${n.count}</span>` : "";
  return `<div class="ncard ${n.read ? "read" : ""} k-${esc(n.kind)}" data-nid="${n.id}">
    ${noteIcon(n)}
    <div class="n-main">
      <div class="n-top">${showApp ? `<span class="n-app">${esc(n.app)}</span>` : ""}<b class="n-title">${esc(t.title)}</b>${cnt}<span class="n-time">${esc(ago(n.at))}</span></div>
      <div class="n-body">${esc(t.body)}</div>
      ${t.sub ? `<div class="n-sub">${n.kind === "done" ? "✓ " : n.kind === "ask" ? "● " : ""}${esc(t.sub)}</div>` : ""}
      ${n.actions?.length ? `<div class="n-acts">${n.actions.map((a) => `<button class="n-act ${esc(a.style)}" data-nact="${esc(a.id)}">${esc(a.label)}</button>`).join("")}</div>`
        : n.done ? `<div class="n-done ${n.done}">${n.done === "block" ? "⛔ 已阻止联网" : "✓ 已允许"}</div>` : ""}
    </div>
    ${close ? `<button class="n-x" data-nclose="${n.id}" title="关闭" aria-label="关闭">✕</button>` : ""}
  </div>`;
}

// how a new note shows: icon (a small app icon at the pill's right end) | inline (the pill's
// content steps aside for one line: icon, sender, text) | card (the pill springs into a card)
const noteStyle = () => sget("notify.style", "icon");
const nfl = { id: 0, until: 0, w: 0, since: 0, anim: false }; // inline note in the pill
let notePop = 0; // newest note: its icon pops in the pill

// small round icon: the sender's picture (inline only, privacy permitting) or the app's icon
function noteMini(n, avatar) {
  if (avatar && n.avatar && noteHidden(n) < 2) return `<img class="nm-av" src="${n.avatar}" alt="">`;
  if (n.iconUrl) return `<img class="app-icon" src="${n.iconUrl}" alt="">`;
  if (n.icon) return iconHtml({ path: n.icon, name: n.app });
  return noteGlyph(n);
}

function inlineNoteHtml() {
  const n = noteById(nfl.id);
  const t = noteText(n);
  const w = Math.max(200, nfl.w - 46);
  const others = notes.filter((x) => !x.read && !x.silent && x.id !== n.id && x.at >= nfl.since).length;
  const html = `<span class="pill-note${nfl.anim ? " in" : ""}" style="width:${w}px">${noteMini(n, true)}<b class="pn-t">${esc(t.title)}</b><span class="pn-b">${esc(t.body)}</span>${others ? `<span class="pn-more mono">+${others}</span>` : ""}</span>`;
  nfl.anim = false;
  return html;
}

// a new note arrived (not muted): announce it the chosen way
function announceNote(n) {
  if (MANAGER || editing) return;
  const st = noteStyle();
  // a note with buttons always springs out as a card (the buttons have to be reachable)
  if (st === "card" || n.actions?.length) { showNote(n.id); return; }
  if (expanded) return; // the notes page / the dot on its icon show it
  if (st === "inline") {
    const showing = nfl.until > Date.now();
    nfl.w = showing ? nfl.w : pill.offsetWidth; // keep the pill's width while it shows notes
    nfl.since = showing ? nfl.since : n.at;
    nfl.id = n.id;
    nfl.anim = true;
    nfl.until = Date.now() + Math.min(30, Math.max(2, Number(sget("notify.duration", 5)) || 5)) * 1000;
  } else {
    notePop = n.id;
    setTimeout(() => { if (notePop === n.id) { notePop = 0; renderPill(); } }, 900);
  }
  renderPill();
}

function renderNote() {
  if (MANAGER) return;
  const list = nst.active.map(noteById).filter(Boolean);
  if (!list.length) { hideNotes(); return; }
  const el = $("note");
  nst.fan = nst.hover && list.length > 1;
  el.innerHTML = nst.fan
    ? `<div class="nfan">${list.slice(0, 4).map((n) => noteCardHtml(n, true)).join("")}${list.length > 4 ? `<div class="n-more">还有 ${list.length - 4} 条，在通知页里</div>` : ""}</div>`
    : noteCardHtml(list[0], nst.hover);
  el.style.width = Math.min(window.innerWidth - 16, NOTE_W) + "px";
  nst.h = Math.ceil(el.offsetHeight);
  const behind = nst.fan ? 0 : Math.min(2, list.length - 1);
  $("note-tail").className = behind ? `n${behind}` : "";
  $("note-tail").innerHTML = "<i></i><i></i>";
  applySize();
}

function showNote(id) {
  if (MANAGER || expanded || editing || fling.active || tour.on) return;
  nst.active = [id, ...nst.active.filter((x) => x !== id)].slice(0, 12);
  if (!nst.on) {
    nst.on = true;
    island.classList.add("noting");
    invoke("note_mode", { on: true }).catch(() => {});
  }
  // a small spring for every new note
  island.classList.remove("n-pop");
  void island.offsetWidth;
  island.classList.add("n-pop");
  renderNote();
  armNoteTimer();
}

function armNoteTimer(ms) {
  clearTimeout(nst.timer);
  if (nst.hover) return;
  const base = Math.min(30, Math.max(2, Number(sget("notify.duration", 5)) || 5)) * 1000;
  // a card waiting for a decision stays up longer
  const waits = noteById(nst.active[0])?.actions?.length;
  nst.timer = setTimeout(hideNotes, ms ?? (waits ? Math.max(base, 15000) : base));
}

// a button on a note card (MicaBar's own notes): act, then show the outcome on the card
function noteAction(btn) {
  const id = Number(btn.closest("[data-nid]")?.dataset.nid);
  const n = noteById(id);
  if (!n || btn.disabled) return;
  const act = btn.dataset.nact;
  btn.closest(".n-acts")?.querySelectorAll("button").forEach((b) => (b.disabled = true));
  const say = (msg, bad) => (MANAGER || expanded ? toast(msg, bad) : fireFlash("fn", msg, bad ? "bad" : "info"));
  if (n.kind !== "firstnet") return;
  invoke("firstnet_action", { path: n.data, action: act, note: id }).then((msg) => {
    n.actions = [];
    n.read = true;
    n.done = act;
    if (msg) say(msg);
    fnLoad(true);
    if (nst.on) { renderNote(); armNoteTimer(2500); }
    afterNotesChange();
  }).catch((e) => {
    btn.closest(".n-acts")?.querySelectorAll("button").forEach((b) => (b.disabled = false));
    say(String(e), true);
  });
}

function hideNotes() {
  clearTimeout(nst.timer);
  nst.active = [];
  nst.hover = false;
  nst.fan = false;
  if (!nst.on) return;
  nst.on = false;
  island.classList.remove("noting", "n-pop");
  $("note-tail").className = "";
  invoke("note_mode", { on: false }).catch(() => {});
  setTimeout(() => { if (!nst.on) $("note").innerHTML = ""; }, 450);
  if (!expanded) applySize();
}

// the edges of the cards behind the front one, just outside the island (top / bottom dock)
function placeTail(r) {
  const t = $("note-tail");
  if (MANAGER || !t) return;
  if (!nst.on || expanded || r.vertical || nst.fan || !t.className) { t.style.display = "none"; return; }
  const n = t.classList.contains("n2") ? 2 : 1;
  t.style.display = r.dock === "bottom" ? "flex" : "block";
  t.style.left = r.x + "px";
  t.style.width = r.w + "px";
  t.style.top = (r.dock === "bottom" ? r.y - n * 7 : r.y + r.h) + "px";
  t.classList.toggle("up", r.dock === "bottom");
}

function readNotes(ids) {
  notes.forEach((n) => { if (ids.includes(n.id)) n.read = true; });
  invoke("notify_read", { ids }).catch(() => {});
}

function openNote(id) {
  const n = noteById(id);
  if (n) n.read = true;
  invoke("notify_open", { id }).catch((e) => toast(String(e), true));
  afterNotesChange();
}

function afterNotesChange() {
  renderPill();
  if (!MANAGER) renderPages();
  const c = $("tab-notes");
  if (c) {
    const u = unreadCount();
    c.textContent = u ? String(u) : "";
    c.parentElement?.classList.toggle("has", u > 0);
  }
  if (tab === "notes") renderList(true);
}

if (!MANAGER) {
  const el = $("note");
  el.addEventListener("mouseenter", () => { nst.hover = true; clearTimeout(nst.timer); renderNote(); });
  el.addEventListener("mouseleave", () => { nst.hover = false; if (nst.on) { renderNote(); armNoteTimer(2500); } });
  el.addEventListener("click", (e) => {
    const act = e.target.closest("[data-nact]");
    if (act) { noteAction(act); return; }
    const x = e.target.closest("[data-nclose]");
    const c = e.target.closest("[data-nid]");
    const id = Number((x || c)?.dataset[x ? "nclose" : "nid"]);
    if (!id) return;
    if (x) readNotes([id]);
    else openNote(id);
    nst.active = nst.active.filter((i) => i !== id);
    renderNote();
    afterNotesChange();
  });
}

listen("note", (e) => {
  const n = e.payload;
  if (!n) return;
  notes = [n, ...notes.filter((x) => x.id !== n.id)].slice(0, 300);
  if (!n.silent) announceNote(n);
  afterNotesChange();
});
// a taskbar flash superseded by the toast of the same message
listen("note-gone", (e) => {
  const id = e.payload;
  notes = notes.filter((x) => x.id !== id);
  if (nst.on) {
    nst.active = nst.active.filter((i) => i !== id);
    renderNote();
  }
  afterNotesChange();
});
listen("notes-read", (e) => {
  const key = e.payload;
  notes.forEach((n) => { if (n.key === key) n.read = true; });
  if (nst.on) {
    nst.active = nst.active.filter((id) => noteById(id)?.key !== key);
    renderNote();
  }
  afterNotesChange();
});
invoke("notify_list").then((l) => { notes = l || []; afterNotesChange(); }).catch(() => {});

// notification centre (a page of the island and of the manager window)
const ncOpen = new Set();
let notifyInfo = null; // { port, quiet, hooks }
let notifyApps = null;
let hookConfirm = null;
let notifyInfoAt = 0;
function loadNotifyInfo() {
  notifyInfoAt = Date.now();
  invoke("notify_hooks").then((i) => { notifyInfo = i; if (tab === "settings" || tab === "notes") renderList(true); }).catch(() => {});
  invoke("notify_apps").then((a) => { notifyApps = a || []; if (tab === "settings") renderList(true); }).catch(() => {});
}

function notesPageHtml() {
  if (!notifyInfo) loadNotifyInfo();
  if (!sget("notify.on", true)) return `<div class="empty">通知已关闭（设置 → 通知）</div>`;
  const unread = unreadCount();
  const head = `<div class="viz-head"><span class="viz-sel">${notes.length ? `今天 ${notes.length} 条${unread ? ` · ${unread} 条未读` : ""}` : "通知"}</span>
    ${notifyInfo?.quiet ? `<span class="tag">勿扰中：只收进这里，不在岛上弹出</span>` : ""}<span class="sp"></span>
    ${notes.length ? `${unread ? `<button class="ghost" data-do="notes-read">全部已读</button>` : ""}<button class="ghost" data-do="notes-clear">全部清除</button>` : ""}</div>`;
  if (!notes.length) {
    return `<div class="viz">${head}<div class="nc-empty">
      <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="${BELL}"/></svg>
      <div class="nc-e1">没有通知</div>
      <div class="m-hint">微信、Telegram、邮件等程序的提醒，Claude Code / Codex 完成任务或等你确认时，会在岛上弹出并收在这里。<br>设置 → 通知 里可以发一条测试通知看看样子。</div></div></div>`;
  }
  const groups = new Map();
  for (const n of notes) {
    if (!groups.has(n.key)) groups.set(n.key, []);
    groups.get(n.key).push(n);
  }
  const body = [...groups.entries()].map(([key, list]) => {
    const open = ncOpen.has(key) || list.length === 1;
    const cards = open
      ? list.slice(0, 40).map((n) => noteCardHtml(n)).join("")
      : noteCardHtml(list[0]) + `<div class="ng-edges n${Math.min(2, list.length - 1)}"><i></i><i></i></div>`;
    const muted = !!(sget("notify.apps", {})?.[key] || {}).mute;
    return `<div class="ng ${open ? "open" : "stacked"}">
      <div class="ng-head" data-ngkey="${esc(key)}"><span class="ng-app">${esc(list[0].app)}</span>${list.length > 1 ? `<span class="ng-n mono">${list.length}</span>` : ""}${muted ? `<span class="tag">已静音</span>` : ""}<span class="sp"></span>
        ${list.length > 1 ? `<button class="link" data-ngtoggle="${esc(key)}">${open ? "收起" : "展开"}</button>` : ""}
        <button class="link" data-ngmute="${esc(key)}" title="${muted ? "恢复这个程序的提醒" : "这个程序的通知只收进这里：不弹出、不亮红点"}">${muted ? "取消静音" : "静音"}</button>
        <button class="ng-x" data-ngclear="${esc(key)}" title="清除这一组" aria-label="清除这一组">✕</button></div>
      <div class="ng-cards" ${open ? "" : `data-ngtoggle="${esc(key)}"`}>${cards}</div></div>`;
  }).join("");
  return `<div class="viz nc">${head}${body}
    <div class="m-hint">点一条打开对应的程序。通知只在内存里，重启 MicaBar 后清空；消息内容不写硬盘、不写日志。</div></div>`;
}

// mute / unmute a program's notes; muting also marks what is already there read (no red dot)
function toggleAppMute(key) {
  const on = !(sget("notify.apps", {})?.[key] || {}).mute;
  const name = notes.find((n) => n.key === key)?.app || key;
  invoke("settings_set", { patch: { notify: { apps: { [key]: { mute: on } } } } })
    .then(() => {
      if (on) {
        const ids = notes.filter((n) => n.key === key && !n.read).map((n) => n.id);
        if (ids.length) readNotes(ids);
      }
      toast(on ? `已静音 ${name}：以后它的通知只收进通知页，不弹出、不亮红点` : `已恢复 ${name} 的提醒`);
      afterNotesChange();
      renderList(true);
    })
    .catch((err) => toast(String(err), true));
}

// right-click on a note or a group header: mute / unmute that program
function notesContext(e) {
  const el = e.target.closest("#list [data-nid], #list [data-ngkey]");
  if (!el) return false;
  e.preventDefault();
  const key = el.dataset.ngkey || noteById(Number(el.dataset.nid))?.key;
  if (!key) return true;
  const name = notes.find((n) => n.key === key)?.app || key;
  const muted = !!(sget("notify.apps", {})?.[key] || {}).mute;
  openMenu(el, `<div class="m-title">${esc(name)}</div>
    <button class="m-item" data-do="note-mute" data-key="${esc(key)}">${muted ? "取消静音" : "静音这个程序（不弹出、不亮红点）"}</button>
    <button class="m-item" data-do="note-clear" data-key="${esc(key)}">清除这一组</button>`);
  return true;
}

function notesClick(e) {
  const act = e.target.closest("#list [data-nact]");
  if (act) { noteAction(act); return true; }
  const tg = e.target.closest("[data-ngtoggle]");
  if (tg && (tg.tagName === "BUTTON" || !e.target.closest("button"))) {
    const k = tg.dataset.ngtoggle;
    if (ncOpen.has(k)) ncOpen.delete(k);
    else ncOpen.add(k);
    renderList(true);
    return true;
  }
  const mu = e.target.closest("[data-ngmute]");
  if (mu) {
    toggleAppMute(mu.dataset.ngmute);
    return true;
  }
  const cl = e.target.closest("[data-ngclear]");
  if (cl) {
    const ids = notes.filter((n) => n.key === cl.dataset.ngclear).map((n) => n.id);
    notes = notes.filter((n) => !ids.includes(n.id));
    invoke("notify_dismiss", { ids }).catch(() => {});
    afterNotesChange();
    return true;
  }
  const c = e.target.closest("#list [data-nid]");
  if (c) {
    openNote(Number(c.dataset.nid));
    return true;
  }
  return false;
}

// ---------- panel ----------

function exitCard(title, r, opts = {}) {
  if (!r) {
    return `<div class="exit-card"><div class="label">${esc(title)}</div><div class="ip mono">—</div><div class="meta">${opts.checking ? "检测中…" : "未检测"}</div></div>`;
  }
  const geo = [r.cc ? country(r.cc) : "", r.asn ? `AS${r.asn}` : "", r.org || ""].filter(Boolean).join(" · ");
  let route = "";
  if (r.node) {
    route = `<div class="route">${r.node.toUpperCase() === "DIRECT" ? "直连 DIRECT" : decorate(r.node)}${r.group ? " · " + decorate(r.group) : ""}</div>`;
  } else if (opts.direct && r.via) {
    route = `<div class="route" style="color:var(--green)">绕过代理 · 网卡 ${esc(r.via.split(" ")[0])}</div>`;
  } else if (opts.direct) {
    route = `<div class="route" style="color:var(--dimmer)">未经代理</div>`;
  }
  const same = opts.sameAs && r.ip && opts.sameAs === r.ip ? `<span class="same">· 与直连相同</span>` : "";
  return `<div class="exit-card">
    <div class="label">${esc(title)} ${same}</div>
    ${r.ip ? `<div class="ip mono">${flagImg(r.cc)}${esc(r.ip)}</div>` : `<div class="ip mono">—</div>`}
    ${geo ? `<div class="meta">${esc(geo)}</div>` : ""}
    ${route}
    ${r.error ? `<div class="error">${esc(r.error)}</div>` : ""}
  </div>`;
}

// Measured proxy groups merged by exit IP: several groups often leave through the same node.
// [{ ip, cc, org, nodes, groups: [g…], isDefault, error }], the default group's exit first.
function exitsByIp(groups) {
  const out = [];
  for (const g of groups || []) {
    const hit = g.ip && out.find((x) => x.ip === g.ip);
    if (hit) {
      hit.groups.push(g);
      if (g.node && !hit.nodes.includes(g.node)) hit.nodes.push(g.node);
      hit.isDefault ||= !!g.isDefault;
    } else {
      out.push({ ip: g.ip, cc: g.cc, org: g.org, error: g.error, nodes: g.node ? [g.node] : [], groups: [g], isDefault: !!g.isDefault });
    }
  }
  return out.sort((a, b) => (b.isDefault - a.isDefault) || (b.groups.length - a.groups.length) || (!!b.ip - !!a.ip));
}

// short tag for one exit in the pill: its default (or first) group, "+2" for the others
function exitTag(x) {
  const main = x.groups.find((g) => g.isDefault) || x.groups[0];
  return shortGroup(main.group, main.isDefault) + (x.groups.length > 1 ? ` +${x.groups.length - 1}` : "");
}

// One row per exit IP: IP · node · ISP, then the groups that use it
function groupExitCard(groups) {
  const exits = exitsByIp(groups);
  const rows = exits.map((x) => {
    const geo = [x.cc ? country(x.cc) : "", x.org || ""].filter(Boolean).join(" · ");
    const chips = x.groups.map((g) => `<span class="gx-chip">${decorate(g.group)}${g.isDefault ? `<span class="tag">默认</span>` : ""}</span>`).join("");
    return `<div class="gx-row">
      <div class="gx-ip mono">${x.ip ? `${flagImg(x.cc)} ${esc(x.ip)}` : `<span class="crit">${esc(x.error ? "检测失败" : "—")}</span>`}</div>
      <div class="gx-meta">${x.nodes.map(decorate).join("、")}${geo ? `${x.nodes.length ? " · " : ""}${esc(geo)}` : ""}</div>
      <div class="gx-groups">${chips}</div>
    </div>`;
  }).join("");
  const n = (groups || []).length;
  return `<div class="exit-card gx"><div class="label">代理出口<span class="gx-sum">${exits.length} 个 IP · ${n} 个分组</span></div>${rows}</div>`;
}

// The exit shown in the pill: the default (fallback) group, else the old single proxy check.
function mainExit(ex, detected) {
  if (!detected) return ex?.direct;
  const d = ex?.groups?.find((g) => g.isDefault) || ex?.groups?.[0];
  return d && (d.ip || d.error) ? d : ex?.proxy;
}

function renderExit() {
  const p = snap?.proxy;
  const ex = exitState || {};
  let html = "";
  if (p?.detected && ex.groups?.length) {
    html += groupExitCard(ex.groups);
    html += exitCard("直连出口", ex.direct, { checking: ex.checking, direct: true });
  } else if (p?.detected) {
    html += exitCard("代理出口", ex.proxy, { checking: ex.checking, sameAs: ex.direct?.ip });
    html += exitCard("直连出口", ex.direct, { checking: ex.checking, direct: true });
  } else {
    html += exitCard("出口 IP（未检测到代理）", ex.direct, { checking: ex.checking, direct: true });
  }
  const spin = ex.checking ? "spin" : "";
  const when = ex.lastCheck ? ago(ex.lastCheck) : "";
  html = `<div class="exit-wrap">${html}
    <button id="diag-btn" class="${diag.problem ? "hot" : ""}" style="${livePhase()}" data-do="diag-open" title="网络诊断：从网卡一路查到节点">${svgI(DG_PATH.pulse, 13, 2.4)}</button>
    <button id="refresh" class="${spin}" title="重新检测出口 IP${when ? "（上次 " + when + "）" : ""}">
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>
    </button></div>`;
  $("exit-row").innerHTML = html;
}

function renderProxyLine() {
  const p = snap?.proxy;
  const parts = [];
  if (!p || !p.detected) {
    parts.push(`<span class="off">未检测到代理软件</span>`);
  } else {
    parts.push(`<b>${esc(p.client || "代理")}</b>${p.coreVersion ? ` <span class="off">${esc(p.coreVersion)}</span>` : ""}`);
    parts.push(p.systemProxy ? `系统代理 <span class="on">${esc(p.systemProxy)}</span>` : `系统代理 <span class="off">关</span>`);
    parts.push(p.tun ? `TUN <span class="on">${esc(p.tun)}</span>` : `TUN <span class="off">关</span>`);
    if (p.localPort) parts.push(`本地端口 <span class="mono">${p.localPort}</span>`);
    if (p.corePids?.length) {
      if (p.mihomoOk) parts.push(`控制接口 <span class="on">已连接</span>`);
      else if (p.mihomoError) parts.push(`控制接口 <span class="bad">${esc(p.mihomoError)}</span>`);
    }
  }
  $("proxy-line").innerHTML = parts.map((x) => `<span>${x}</span>`).join("");
}

function matches(app, c) {
  if (!query) return true;
  const q = query.toLowerCase();
  const hay = [app.name, app.exe, c?.host, c?.remote, c?.remotePort, c?.localPort, c?.node, c?.group, c?.cc]
    .filter((x) => x !== undefined && x !== null)
    .join(" ")
    .toLowerCase();
  return hay.includes(q);
}

// ---------- exit control helpers ----------

const TARGET_LABEL = { proxy: "代理", direct: "直连", reject: "拦截" };
let refs = []; // [app, conn] per rendered connection row, for the ⋯ menu
let menuState = null;
let nodesCache = null;
let confirmDisable = false;
let toastTimer = null;

function ctrl() { return snap?.control || { status: "unavailable", rules: [] }; }

function mainDomain(host) {
  const p = String(host || "").replace(/\.$/, "").split(".");
  if (p.length <= 2) return host;
  const sld = ["com", "net", "org", "gov", "edu", "co", "ac"];
  const take = p[p.length - 1].length === 2 && sld.includes(p[p.length - 2]) ? 3 : 2;
  return p.slice(-take).join(".");
}

function findRule(kind, process, value) {
  const lc = (s) => String(s || "").toLowerCase();
  return ctrl().rules.find((r) => r.kind === kind && lc(r.process) === lc(process) && lc(r.value) === lc(value));
}

function ruleText(r) {
  switch (r.kind) {
    case "process": return `程序 <b>${esc(r.process)}</b>`;
    case "domain": return `域名 <b>${esc(r.value)}</b>`;
    case "suffix": return `主域 <b>*.${esc(r.value)}</b>`;
    case "ip": return `IP <b class="mono">${esc(r.value)}</b>`;
    case "process_domain": return `<b>${esc(r.process)}</b> 访问 <b>${esc(r.value)}</b>`;
    case "process_suffix": return `<b>${esc(r.process)}</b> 访问 <b>*.${esc(r.value)}</b>`;
    case "process_ip": return `<b>${esc(r.process)}</b> 访问 <b class="mono">${esc(r.value)}</b>`;
  }
  return esc(r.kind);
}

function toast(msg, bad) {
  const el = $("toast");
  el.innerHTML = esc(msg);
  el.className = bad ? "show bad" : "show";
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.className = ""), 2600);
}

function closeMenu() {
  menuState = null;
  $("menu").hidden = true;
}

function openMenu(anchor, html) {
  const menu = $("menu");
  menu.innerHTML = html;
  menu.hidden = false;
  const pr = $("panel").getBoundingClientRect();
  const ar = anchor.getBoundingClientRect();
  const mw = menu.offsetWidth, mh = menu.offsetHeight;
  let left = ar.right - pr.left - mw;
  let top = ar.bottom - pr.top + 4;
  if (top + mh > pr.height - 6) top = Math.max(6, ar.top - pr.top - mh - 4);
  menu.style.left = Math.max(6, left) + "px";
  menu.style.top = top + "px";
}

// One row of target buttons for a rule scope.
function targetRow(label, kind, process, value) {
  const cur = findRule(kind, process, value);
  const node = ctrl().node;
  const btn = (t) => {
    const on = cur && cur.target === t;
    const title = t === "proxy" && node ? ` title="MicaBar 节点：${esc(node)}"` : "";
    return `<button class="mt t-${t} ${on ? "on" : ""}"${title} data-set='${esc(JSON.stringify({ kind, process, value, target: on ? "default" : t }))}'>${TARGET_LABEL[t]}</button>`;
  };
  return `<div class="m-row"><div class="m-label">${label}</div><div class="m-btns">${btn("proxy")}${btn("direct")}${btn("reject")}</div></div>`;
}

function setupPrompt() {
  const c = ctrl();
  if (c.status === "unavailable") {
    return `<div class="m-note">${esc(c.message || "出口控制需要 Clash Party / Mihomo Party。")}</div>`;
  }
  if (c.status === "working") return `<div class="m-note">${esc(c.message || "处理中…")}</div>`;
  if (c.status === "pending") return `<div class="m-note">${esc(c.message || "")}</div>`;
  return `<div class="m-note">${c.message && c.status === "available" ? esc(c.message) : "要按程序 / 域名指定出口，需要先给 Clash Party 加一个全局覆写，并重启它一次（断网约 3 秒）。"}${
    c.status === "error" ? `<div class="m-err">${esc(c.message || "")}</div>` : ""
  }</div><div class="m-actions"><button class="primary" data-do="enable">启用出口控制</button></div>`;
}

function appMenu(app) {
  if (ctrl().status !== "active") return `<div class="m-title">${esc(app.name)}</div>${setupPrompt()}`;
  return `<div class="m-title">整个程序走哪个出口</div>
    ${targetRow(`<b>${esc(app.exe)}</b>`, "process", app.exe, null)}
    <div class="m-hint">再点一次已选中的按钮 = 恢复按规则</div>`;
}

function connMenu(app, c) {
  if (ctrl().status !== "active") return `<div class="m-title">${esc(c.host || c.remote || "")}</div>${setupPrompt()}`;
  const rows = [];
  if (c.host) {
    const md = mainDomain(c.host);
    rows.push(targetRow(`域名 <b>${esc(c.host)}</b>`, "domain", null, c.host));
    if (md !== c.host) rows.push(targetRow(`主域 <b>*.${esc(md)}</b>`, "suffix", null, md));
    rows.push(targetRow(`仅 <b>${esc(app.exe)}</b> 访问 <b>*.${esc(md)}</b>`, "process_suffix", app.exe, md));
  } else if (c.remote) {
    rows.push(targetRow(`IP <b class="mono">${esc(c.remote)}</b>`, "ip", null, c.remote));
    rows.push(targetRow(`仅 <b>${esc(app.exe)}</b> 访问这个 IP`, "process_ip", app.exe, c.remote));
  }
  return `<div class="m-title">这条连接走哪个出口</div>${rows.join("")}<div class="m-hint">再点一次已选中的按钮 = 恢复按规则</div>`;
}

function exitChip(app) {
  const st = ctrl().status;
  if (st === "unavailable") return "";
  // the proxy client itself has no exit to choose: keep its place so the columns line up
  if (app.isProxy) return `<span class="exit-chip ph" aria-hidden="true">出口</span>`;
  const r = findRule("process", app.exe, null);
  const t = r ? r.target : "";
  return `<button class="exit-chip ${t ? "t-" + t : ""}" data-chip="${esc(app.key)}" title="选择这个程序的出口">${t ? TARGET_LABEL[t] : "出口"}</button>`;
}

function rulesTabHtml() {
  const c = ctrl();
  if (c.status !== "active") return `<div class="rules-setup">${setupPrompt()}</div>`;
  const nodeList = nodesCache
    ? `<div class="node-list">${nodesCache.nodes.map((n) => {
        const d = n.delay ? `<span class="delay ${n.delay < 200 ? "good" : n.delay < 500 ? "mid" : "bad"}">${n.delay} ms</span>` : `<span class="delay">—</span>`;
        return `<div class="node-item ${n.name === nodesCache.now ? "now" : ""}" data-node="${esc(n.name)}"><span>${decorate(n.name)}</span>${d}</div>`;
      }).join("")}</div>`
    : "";
  const rules = c.rules.length
    ? c.rules.slice().reverse().map((r) => `<div class="rule"><span class="rule-t t-${r.target}">${TARGET_LABEL[r.target]}</span><span class="rule-d">${ruleText(r)}</span><button class="rule-x" data-rm="${esc(r.id)}" title="删除">✕</button></div>`).join("")
    : `<div class="empty">还没有出口规则。在「连接」页点程序右边的「出口」或连接右边的「⋯」来添加。</div>`;
  return `<div class="node-bar">
      <span>「代理」使用节点</span><b class="node-now">${c.node ? decorate(c.node) : "—"}</b>
      <button data-do="nodes">${nodesCache ? "收起" : "换节点"}</button>
      ${nodesCache ? `<button data-do="test">测速</button>` : ""}
    </div>${nodeList}
    <div class="rules">${rules}</div>
    <div class="m-actions end"><button class="${confirmDisable ? "danger" : "ghost"}" data-do="disable">${confirmDisable ? "再点一次：移除覆写并重启 Clash Party（代理会断开几秒）" : "停用出口控制"}</button></div>`;
}

function bannerHtml() {
  const c = ctrl();
  let dismissed = false;
  try { dismissed = localStorage.getItem("ni-setup-dismissed") === "1"; } catch { /* ignore */ }
  if (c.status === "working") return `<div class="banner">${esc(c.message || "处理中…")}</div>`;
  if (c.status === "error") return `<div class="banner bad">${esc(c.message || "出错了")} <button data-do="enable">重试</button></div>`;
  if (c.status === "available" && c.message) {
    return `<div class="banner">${esc(c.message)} <button class="primary" data-do="enable">更新</button></div>`;
  }
  if (c.status === "available" && !dismissed) {
    return `<div class="banner">检测到 ${esc(c.client || "Clash Party")}：可以启用「出口控制」，按程序 / 域名指定走代理、直连或拦截。
      <button class="primary" data-do="enable">启用</button><button class="ghost" data-do="dismiss">不再提示</button></div>`;
  }
  return "";
}

// ---------- history buffers (filled from snapshots) ----------

const HIST_MAX = 300; // 5 minutes at one snapshot per second
const speedHist = { t: [], up: [], down: [] };
const appHist = new Map(); // key -> { name, path, exe, up: [], down: [] } aligned with speedHist

function recordSpeed(s) {
  speedHist.t.push(s.ts);
  speedHist.up.push(s.up || 0);
  speedHist.down.push(s.down || 0);
  const len = speedHist.t.length;
  const seen = new Set();
  for (const a of s.apps || []) {
    if (a.isProxy) continue;
    let h = appHist.get(a.key);
    if (!h) {
      h = { name: a.name, path: a.path, exe: a.exe, up: new Array(len - 1).fill(0), down: new Array(len - 1).fill(0) };
      appHist.set(a.key, h);
    }
    h.up.push(a.up || 0);
    h.down.push(a.down || 0);
    seen.add(a.key);
  }
  for (const [k, h] of appHist) {
    if (!seen.has(k)) {
      h.up.push(0);
      h.down.push(0);
    }
  }
  if (len > HIST_MAX) {
    const cut = len - HIST_MAX;
    for (const arr of [speedHist.t, speedHist.up, speedHist.down]) arr.splice(0, cut);
    for (const [k, h] of appHist) {
      h.up.splice(0, cut);
      h.down.splice(0, cut);
      if (!h.up.some((v) => v > 0) && !h.down.some((v) => v > 0)) appHist.delete(k);
    }
  }
}

// flag emoji (regional-indicator pairs) can't be drawn in SVG text on Windows: drop them there
const noFlags = (s) => String(s ?? "").replace(/[\u{1F1E6}-\u{1F1FF}]{2}\s*/gu, "");
const hms = (t) => new Date(t).toLocaleTimeString("zh-CN", { hour12: false });
const hm = (t) => new Date(t).toLocaleTimeString("zh-CN", { hour12: false, hour: "2-digit", minute: "2-digit" });

function fmtBytes(b) {
  if (!b || b < 1) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return `${b.toFixed(b < 10 && i ? 1 : 0)} ${u[i]}`;
}
const tickRate = (v) => (v === 0 ? "0" : fmtBytes(v).replace(" ", ""));

function segmented(name, value, options) {
  return `<div class="segctl">${options.map(([v, t]) => `<button data-seg="${name}" data-val="${v}" class="${String(v) === String(value) ? "on" : ""}">${t}</button>`).join("")}</div>`;
}

// ---------- speed tab ----------

let speedRange = 60;
let speedApp = null;

// rolling-counter text for a rate: number and unit apart ("547" + "KB/s"), zero as "0 B/s"
function odoRate(b) {
  const t = b >= 1 ? fmtRate(b) : "0 B/s";
  const i = t.lastIndexOf(" ");
  return [t.slice(0, i), t.slice(i + 1)];
}

let speedView = null; // { xs, up, down } of the big chart as last drawn: the scrubbed index points into it

// Big numbers above the chart: the latest second, or the one under the crosshair while scrubbing.
function speedRead(i) {
  const v = speedView;
  if (!v || !v.xs.length) return;
  const k = i == null ? v.xs.length - 1 : Math.min(i, v.xs.length - 1);
  Odo.set("sp-down", ...odoRate(v.down[k]));
  Odo.set("sp-up", ...odoRate(v.up[k]));
  const w = document.querySelector(".sp-when-t");
  // the crosshair carries the clock time; up here: how long ago
  const back = Math.round((v.xs[v.xs.length - 1] - v.xs[k]) / 1000);
  if (w) w.textContent = i == null || back <= 0 ? "现在" : back < 60 ? `${back} 秒前` : `${Math.floor(back / 60)} 分 ${back % 60} 秒前`;
  document.querySelector(".sp-hero")?.classList.toggle("scrubbing", i != null);
}
Charts.onScrub("speed", (i) => speedRead(i));

function speedTabHtml() {
  const n = Math.min(speedRange, speedHist.t.length);
  if (n < 2) return `<div class="empty">正在收集网速数据…</div>`;
  const src = speedApp ? appHist.get(speedApp) : null;
  if (speedApp && !src) speedApp = null;
  const xs = speedHist.t.slice(-n);
  const up = (src ? src.up : speedHist.up).slice(-n);
  const down = (src ? src.down : speedHist.down).slice(-n);
  speedView = { xs, up, down };
  const chart = Charts.line({
    id: "speed", width: chartW() - 28, height: 140, xs,
    series: [{ name: "下载", cls: "s1", values: down }, { name: "上传", cls: "s2", values: up }],
    fmtY: (v) => fmtRate(v), fmtTick: tickRate, fmtX: hms, binary: true, area: true, scrub: true, slide: true,
  });
  const peakD = Math.max(...down, 0), peakU = Math.max(...up, 0);
  const apps = [...appHist.entries()]
    .map(([key, h]) => ({ key, ...h, cur: (h.up[h.up.length - 1] || 0) + (h.down[h.down.length - 1] || 0), sum: h.up.slice(-n).reduce((a, b) => a + b, 0) + h.down.slice(-n).reduce((a, b) => a + b, 0) }))
    .filter((a) => a.sum > 0)
    .sort((a, b) => b.cur - a.cur || b.sum - a.sum)
    .slice(0, 15);
  const rows = apps.map((a) => {
    const series = a.up.slice(-n).map((u, i) => u + a.down.slice(-n)[i]);
    return `<div class="sp-row ${a.key === speedApp ? "sel" : ""} ${a.key === sheetApp ? "open" : ""}" data-sheet-app="${esc(a.key)}" tabindex="0" title="点一下看详情">
      ${iconHtml(a)}<span class="sp-name">${esc(a.name)}</span>${Charts.spark(series, 110, 24, "s1", true)}
      <span class="speed mono">${speedHtml(a.up[a.up.length - 1], a.down[a.down.length - 1])}</span><span class="chev">›</span></div>`;
  }).join("");
  const range = { 60: "1 分钟", 120: "2 分钟", 300: "5 分钟" }[speedRange] || "";
  return `<div class="viz sp-viz">
    <div class="viz-head">${segmented("srange", speedRange, [[60, "1 分钟"], [120, "2 分钟"], [300, "5 分钟"]])}
      <span class="viz-sel">${src ? `只看 <b>${esc(src.name)}</b> <button data-sapp="">✕ 看全部</button>` : "全部程序"}</span></div>
    <div class="hero-wrap"><i class="hero-glow"></i>
      <div class="sp-hero glow-card">
        <div class="sp-read">
          <div class="sp-num"><span class="sp-k"><i class="key s1"></i>下载</span>${Odo.slot("sp-down", "big")}</div>
          <div class="sp-num"><span class="sp-k"><i class="key s2"></i>上传</span>${Odo.slot("sp-up", "big")}</div>
          <div class="sp-when"><span class="sp-when-t">现在</span><span class="sp-peak">${range}峰值 ↓${esc(fmtRate(peakD) || "0")} · ↑${esc(fmtRate(peakU) || "0")}</span></div>
        </div>
        ${chart}
      </div>
    </div>
    <div class="g-head">按程序<span class="g-note">点一下看详情</span></div>
    <div class="sp-rows cascade-items">${rows || `<div class="empty">这段时间没有程序在传数据</div>`}</div>
  </div>`;
}

// ---------- one program's details (bottom sheet over the speed page) ----------

let sheetApp = null;

function appSheetHtml(key) {
  const h = appHist.get(key);
  if (!h) return "";
  const n = Math.min(speedRange, speedHist.t.length, h.up.length);
  const xs = speedHist.t.slice(-n), up = h.up.slice(-n), down = h.down.slice(-n);
  const sum = (a) => a.reduce((s, v) => s + v, 0);
  const live = snap?.apps?.find((a) => a.key === key);
  const w = Math.max(300, (Sheet.body()?.clientWidth || $("panel").clientWidth - 16) - 32);
  const range = { 60: "1 分钟", 120: "2 分钟", 300: "5 分钟" }[speedRange] || "这段时间";
  const stat = (k, v) => `<div class="as-stat"><span>${k}</span><b>${v}</b></div>`;
  return `<div class="as-head" data-sheet-drag>${iconHtml(h)}<div class="as-title"><b>${esc(h.name)}</b><span>${esc(h.path || h.exe || "")}</span></div>
      <button class="ghost" data-sheet-do="close" title="关闭（Esc）">✕</button></div>
    <div class="as-read">
      <div class="sp-num"><span class="sp-k"><i class="key s1"></i>下载</span>${Odo.slot("as-down", "big")}</div>
      <div class="sp-num"><span class="sp-k"><i class="key s2"></i>上传</span>${Odo.slot("as-up", "big")}</div>
    </div>
    ${n >= 2 ? Charts.line({ id: "app-sheet", width: w, height: 120, xs, series: [{ name: "下载", cls: "s1", values: down }, { name: "上传", cls: "s2", values: up }], fmtY: (v) => fmtRate(v), fmtTick: tickRate, fmtX: hms, binary: true, area: true }) : ""}
    <div class="as-stats">
      ${stat(`${range}下载`, fmtBytes(sum(down)))}
      ${stat(`${range}上传`, fmtBytes(sum(up)))}
      ${stat("下载峰值", fmtRate(Math.max(...down, 0)) || "0")}
      ${stat("上传峰值", fmtRate(Math.max(...up, 0)) || "0")}
      ${stat("当前连接", live ? live.conns.length : 0)}
      ${live ? stat("出口", live.viaProxy ? "代理" : "直连") : ""}
    </div>
    <div class="m-actions end">
      <button class="ghost" data-sheet-do="conns">看它的连接</button>
      <button class="primary" data-sheet-do="focus">${speedApp === key ? "回到全部程序" : "在大图里只看它"}</button>
    </div>`;
}

function sheetSync() {
  if (!sheetApp || !Sheet.isOpen()) return;
  const h = appHist.get(sheetApp);
  if (!h) return;
  Sheet.update(appSheetHtml(sheetApp));
  const body = Sheet.body();
  if (body && !body._charts) { Charts.attach(body); body._charts = true; }
  if (body) Charts.restore(body);
  Odo.set("as-down", ...odoRate(h.down[h.down.length - 1]));
  Odo.set("as-up", ...odoRate(h.up[h.up.length - 1]));
}

function openAppSheet(key) {
  if (!appHist.get(key)) return;
  sheetApp = key;
  diagSheetOpen = false;
  Sheet.open($("panel"), "", {
    onClose: () => { sheetApp = null; document.querySelectorAll(".sp-row.open").forEach((r) => r.classList.remove("open")); measurePanel(); },
    onClick: (e) => {
      const b = e.target.closest("[data-sheet-do]");
      if (!b) return;
      const act = b.dataset.sheetDo;
      const key = sheetApp;
      if (act === "close") Sheet.close();
      else if (act === "focus") {
        speedApp = speedApp === key ? null : key;
        Sheet.close();
        renderList(true);
      } else if (act === "conns") {
        const name = appHist.get(key)?.name || "";
        Sheet.close();
        switchTab("conns");
        $("search").value = name;
        query = name;
        renderList(true);
      }
    },
  });
  document.querySelectorAll(".sp-row").forEach((r) => r.classList.toggle("open", r.dataset.sheetApp === key));
  measurePanel();
  sheetSync();
}

// after every rebuild of the list: tilt, cascade and the live numbers continue on the new elements
function afterRender() {
  Tilt.restore();
  Cascade.after();
  if (tab === "speed") {
    speedRead(Charts.scrubIndex("speed"));
    sheetSync();
  } else if (Sheet.isOpen() && !diagSheetOpen) {
    Sheet.close();
  }
}

// ---------- traffic tab ----------

let trafficDays = 1;
let trafficDay = null; // one day picked in the chart ("YYYY-MM-DD"): the app list shows only that day
let trafficSort = "total";
try { trafficSort = localStorage.getItem("mb-traffic-sort") || "total"; } catch { /* ignore */ }
let trafficData = null;
let trafficAt = 0;

function loadTraffic(force) {
  if (!force && Date.now() - trafficAt < 10_000) return;
  trafficAt = Date.now();
  invoke("traffic_query", { days: trafficDays, day: trafficDays > 1 ? trafficDay : null })
    .then((r) => { trafficData = r; if (tab === "traffic" || tab === "home") renderList(true); })
    .catch(() => {});
}

const dayName = (date, last) => {
  if (date === last) return "今天";
  const d = new Date(date + "T00:00:00");
  return `${d.getMonth() + 1}月${d.getDate()}日 周${"日一二三四五六"[d.getDay()]}`;
};

function trafficTabHtml() {
  loadTraffic(false);
  const r = trafficData;
  if (!r) return `<div class="empty">正在读取流量记录…</div>`;
  const P = (x) => x.pu + x.pd, D = (x) => x.du + x.dd;
  const last = r.days[r.days.length - 1]?.date;
  const picked = trafficDays > 1 && trafficDay ? r.days.find((d) => d.date === trafficDay) : null;
  // totals: the picked day, or the whole range
  const span = picked ? [picked] : r.days;
  const tp = span.reduce((s, d) => s + P(d), 0), td = span.reduce((s, d) => s + D(d), 0);
  const tot = tp + td;
  const pct = (v) => (tot ? Math.round((v / tot) * 100) : 0);
  const rangeLabel = picked ? dayName(picked.date, last) : trafficDays > 1 ? `最近 ${trafficDays} 天` : "今天";
  const head = `<div class="viz-head">${segmented("tdays", trafficDays, [[1, "今天"], [7, "7 天"], [30, "30 天"]])}
    <span class="legend"><span class="lg"><i class="key s1"></i>代理 ${fmtBytes(tp)}（${pct(tp)}%）</span><span class="lg"><i class="key s2"></i>直连 ${fmtBytes(td)}（${pct(td)}%）</span></span></div>`;
  const used = r.days.filter((d) => P(d) + D(d) > 0);
  const avg = trafficDays > 1 && used.length ? r.days.reduce((s, d) => s + P(d) + D(d), 0) / used.length : 0;
  const peak = trafficDays > 1 && used.length ? used.reduce((a, b) => (P(b) + D(b) > P(a) + D(a) ? b : a)) : null;
  const summary = `<div class="t-total"><span class="t-label">${esc(rangeLabel)}</span><span class="t-value">${fmtBytes(tot)}</span>
    ${Charts.segbar([{ value: tp, cls: "s1" }, { value: td, cls: "s2" }], `代理 ${fmtBytes(tp)} · 直连 ${fmtBytes(td)}`)}</div>
    ${!picked && peak ? `<div class="t-stats"><span>有流量的 ${used.length} 天平均 <b class="mono">${fmtBytes(avg)}</b>/天</span><span>最多 <button class="link" data-tday="${esc(peak.date)}">${esc(dayName(peak.date, last))}</button> <b class="mono">${fmtBytes(P(peak) + D(peak))}</b></span></div>` : ""}`;
  const chart = trafficDays > 1
    ? Charts.columns({
        id: "traffic", width: chartW(), height: trafficDays > 7 ? 170 : 160, cats: r.days.map((d) => d.date),
        stacks: [{ name: "代理", cls: "s1", values: r.days.map(P) }, { name: "直连", cls: "s2", values: r.days.map(D) }],
        fmtY: fmtBytes, fmtTick: tickRate, binary: true, fmtCat: (d) => (d === last ? "今天" : d.slice(5)),
        pick: "tday", selected: picked?.date ?? null,
      })
    : "";
  const key = { total: (a) => P(a) + D(a), proxy: P, direct: D, up: (a) => a.pu + a.du, down: (a) => a.pd + a.dd }[trafficSort] || ((a) => P(a) + D(a));
  const apps = r.apps.filter((a) => key(a) > 0).sort((a, b) => key(b) - key(a));
  const rows = apps.slice(0, 40).map((a) => `<div class="t-row">
      ${iconHtml({ path: a.path, name: a.name })}<span class="sp-name">${esc(a.name)}</span>
      ${Charts.segbar([{ value: P(a), cls: "s1" }, { value: D(a), cls: "s2" }], `代理 ${fmtBytes(P(a))} · 直连 ${fmtBytes(D(a))}`)}
      <span class="t-num mono">${fmtBytes(key(a))}</span>
      <span class="t-sub mono">↑${fmtBytes(a.pu + a.du)} ↓${fmtBytes(a.pd + a.dd)}</span></div>`).join("");
  const appHead = `<div class="g-head t-apphead">按程序（${esc(rangeLabel)}${picked || trafficDays === 1 ? "" : "合计"}）
    ${picked ? `<button class="link" data-tday="">看全部 ${trafficDays} 天</button>` : ""}<span class="sp"></span>
    ${segmented("tsort", trafficSort, [["total", "总量"], ["proxy", "代理"], ["direct", "直连"], ["down", "下载"], ["up", "上传"]])}</div>`;
  return `<div class="viz">${head}${summary}${chart}
    ${trafficDays > 1 && !picked ? `<div class="m-hint t-pick">点柱子看那一天的明细</div>` : ""}
    ${appHead}
    ${rows || `<div class="empty">${picked ? "这一天没有流量记录" : "还没有流量记录"}</div>`}
    ${apps.length > 40 ? `<div class="m-hint">只列出前 40 个程序（共 ${apps.length} 个）</div>` : ""}
    <div class="m-hint">「代理」= 经过代理节点的字节；其余算「直连」。数据每分钟存一次，存在本机，保留最近 30 天，更早的自动删除。</div></div>`;
}

// ---------- node quality tab ----------

let latHist = [];
let latAt = -1;
let nodeRange = 120; // minutes

function loadLatency() {
  const at = snap?.latency?.at || 0;
  if (at === latAt) return;
  latAt = at;
  invoke("latency_history").then((h) => { latHist = h || []; if (tab === "nodes" || tab === "home") renderList(true); }).catch(() => {});
}

// colour per node, kept for the session (colour follows the node, not its rank); four slots
const nodeColor = new Map();
function nodeCls(node) {
  if (!nodeColor.has(node)) {
    const used = new Set(nodeColor.values());
    const free = ["s1", "s2", "s3", "s4"].find((c) => !used.has(c));
    if (!free) return null;
    nodeColor.set(node, free);
  }
  return nodeColor.get(node);
}

function latStats(samples) {
  const ok = samples.filter((s) => s.ms != null).map((s) => s.ms).sort((a, b) => a - b);
  return {
    n: samples.length,
    avg: ok.length ? Math.round(ok.reduce((a, b) => a + b, 0) / ok.length) : null,
    p95: ok.length ? ok[Math.min(ok.length - 1, Math.floor(ok.length * 0.95))] : null,
    loss: samples.length ? Math.round(((samples.length - ok.length) / samples.length) * 100) : null,
  };
}

function nodesTabHtml() {
  loadLatency();
  const L = snap?.latency;
  const now = Date.now();
  // the nodes of the latest round (main exit first), then any others still in range
  const lastAt = latHist.reduce((m, s) => Math.max(m, s.at), 0);
  const current = latHist.filter((s) => s.at === lastAt).map((s) => s.node);
  const pts = latHist.filter((s) => nodeRange >= 100000 || s.at > now - nodeRange * 60_000);
  const nodes = [...new Set([...current, ...pts.map((s) => s.node)])].filter((n) => nodeCls(n));
  const hourOf = (n) => latHist.filter((s) => s.node === n && s.at > now - 3600_000);
  const main = latStats(hourOf(L?.node));
  const tile = (label, value, note = "") => `<div class="tile"><div class="tile-label">${label}</div><div class="tile-value">${value}</div>${note ? `<div class="tile-note">${note}</div>` : ""}</div>`;
  const tiles = `<div class="tiles">
    ${tile("主出口当前延迟", L?.at ? (L.ms != null ? `${L.ms} ms` : `<span class="crit">✕ ${esc(L.error || "超时")}</span>`) : "—", L?.node ? decorate(L.node) : "")}
    ${tile("1 小时平均", main.avg != null ? `${main.avg} ms` : "—")}
    ${tile("1 小时 P95", main.p95 != null ? `${main.p95} ms` : "—")}
    ${tile("1 小时丢包", main.loss != null ? `${main.loss}%` : "—", main.n ? `${main.n} 次测速` : "")}
  </div>`;
  // one line per node over the shared round timestamps
  const ats = [...new Set(pts.map((s) => s.at))].sort((a, b) => a - b);
  const byKey = new Map(pts.map((s) => [`${s.node}|${s.at}`, s]));
  const series = nodes.map((n) => ({ name: noFlags(n), cls: nodeCls(n), values: ats.map((t) => byKey.get(`${n}|${t}`)?.ms ?? null) }));
  const chart = ats.length >= 2
    ? Charts.line({
        id: "latency", width: chartW(), height: 170, xs: ats.map((at) => ({ at })),
        series,
        markers: ats.map((t, i) => {
          const down = nodes.filter((n) => byKey.get(`${n}|${t}`)?.ms === null);
          return down.length ? { i, label: `${down.map(noFlags).join("、")} 无响应` } : null;
        }).filter(Boolean),
        ref: { value: sget("flash.latencyMs", 800), label: `提示阈值 ${sget("flash.latencyMs", 800)} ms` },
        fmtY: (v) => `${Math.round(v)} ms`, fmtTick: (v) => `${v}`, fmtX: (x) => hm(x.at),
      })
    : `<div class="empty">${snap?.proxy?.mihomoOk ? "测速数据还不够，稍等一会儿" : "需要 mihomo 控制接口和代理出口节点才能测速"}</div>`;
  // per node: which groups use it, now / average / P95 / loss over the last hour
  const groupsOf = (n) => (exitState?.groups || []).filter((g) => g.node === n).map((g) => shortGroup(g.group, g.isDefault));
  const table = nodes.length > 1 ? `<div class="lat-table">
      <div class="lt-row lt-head"><span>节点</span><span>现在</span><span>平均</span><span>P95</span><span>丢包</span></div>
      ${nodes.map((n) => {
        const st = latStats(hourOf(n));
        const cur = byKey.get(`${n}|${lastAt}`) || latHist.filter((s) => s.node === n).pop();
        const g = groupsOf(n);
        return `<div class="lt-row"><span class="lt-node"><i class="key ${nodeCls(n)}"></i>${decorate(n)}${g.length ? `<span class="lt-groups">${esc(g.join("、"))}</span>` : ""}</span>
          <span class="mono">${cur ? (cur.ms != null ? `${cur.ms} ms` : `<span class="crit">✕</span>`) : "—"}</span>
          <span class="mono">${st.avg != null ? `${st.avg} ms` : "—"}</span><span class="mono">${st.p95 != null ? `${st.p95} ms` : "—"}</span>
          <span class="mono">${st.loss != null ? `${st.loss}%` : "—"}</span></div>`;
      }).join("")}
    </div>` : "";
  return `<div class="viz">
    <div class="viz-head">${segmented("nrange", nodeRange, [[30, "30 分钟"], [120, "2 小时"], [100000, "全部"]])}
      <span class="viz-sel">各出口节点的延迟（${{ saver: 60, balanced: 30, smooth: 15 }[sget("general.profile", "balanced")]} 秒测一次）</span>
      <button class="ghost" data-do="lat-probe">立即测速</button></div>
    ${tiles}${chart}${table}
    <div class="m-hint">每个分组正在用的节点都会测（最多 4 个）。✕ 表示那次测速没有响应。主出口（默认分组）无响应 / 延迟超过阈值时会在胶囊上提示（设置里可改）。</div></div>`;
}

// ---------- AI quota page ----------

function untilText(ms) {
  if (!ms) return "";
  const s = Math.round((ms - Date.now()) / 1000);
  if (s <= 0) return "已重置";
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  const left = d ? `${d} 天 ${h} 小时` : h ? `${h} 小时 ${m} 分` : `${Math.max(1, m)} 分钟`;
  const at = new Date(ms);
  const day = ["日", "一", "二", "三", "四", "五", "六"][at.getDay()];
  const same = at.toDateString() === new Date().toDateString();
  return `${left}后重置（${same ? "今天" : "周" + day} ${hm(ms)}）`;
}

// one meter per usage window: the share used, coloured only past the warning thresholds
// Models that share one quota (same share used, same reset) become one row: Antigravity lists
// every model and variant ("Gemini 4 Neon (High)", "(Low)"…), mostly drawing from a few pools.
function groupWindows(ws) {
  const groups = new Map();
  for (const w of ws) {
    const key = `${Math.round(w.used)}|${w.resetsAt ? Math.round(w.resetsAt / 60000) : ""}`;
    if (!groups.has(key)) groups.set(key, { used: w.used, resetsAt: w.resetsAt, members: [] });
    groups.get(key).members.push(w);
  }
  return [...groups.values()].map((g) => {
    const labels = g.members.map((w) => w.label);
    const families = [...new Set(labels.map((l) => l.replace(/\s*[(（].*$/, "").trim() || l))];
    const label = labels.length === 1 ? labels[0] : families.length === 1 ? families[0] : `${families[0]} 等`;
    const count = labels.length === 1 ? "" : families.length === 1 ? `${labels.length} 档` : `${labels.length} 个模型`;
    return { label, count, used: g.used, resetsAt: g.resetsAt, members: g.members, title: labels.join("、") };
  }).sort((a, b) => b.used - a.used);
}

const aiExpanded = new Set();
const GROUP_FROM = 6; // this many windows or more (Antigravity's per-model list): grouped, 3 rows until expanded

function quotaBars(q, compact, ringed = []) {
  let rows = q.windows.map((w) => ({ ...w, members: [w], title: w.label }));
  if (q.windows.length >= GROUP_FROM) rows.sort((a, b) => b.used - a.used); // expanded: busiest first
  let more = "";
  if (q.windows.length >= GROUP_FROM && !(aiExpanded.has(q.tool) && !compact)) {
    const groups = groupWindows(q.windows);
    rows = groups.slice(0, 3);
    const hidden = groups.slice(3).reduce((n, g) => n + g.members.length, 0);
    if (!compact) {
      more = `<button class="q-more" data-ai-more="${esc(q.tool)}">${hidden ? `还有 ${hidden} 个模型，` : ""}展开全部 ${q.windows.length} 个模型</button>`;
    }
  } else if (q.windows.length >= GROUP_FROM && !compact) {
    more = `<button class="q-more" data-ai-more="${esc(q.tool)}">收起</button>`;
  }
  return rows.map((w) => {
    const pct = Math.max(0, Math.min(100, w.used));
    const pos = ringed.findIndex((r) => w.members.includes(r));
    const where = pos < 0 ? "" : ringed.length > 1 ? (pos ? " · 内圈" : " · 外圈") : " · 环";
    return `<div class="qrow ${compact ? "compact" : ""}">
      <span class="q-name" title="${esc(w.title + where)}"><span class="q-lab">${esc(w.label)}</span>${w.count ? `<span class="q-cnt">${esc(w.count)}</span>` : ""}${where ? `<span class="q-where">${where}</span>` : ""}</span>
      <span class="meter" role="meter" aria-valuenow="${Math.round(pct)}" aria-valuemin="0" aria-valuemax="100"><i class="${quotaClass(pct)}" style="width:${pct}%"></i></span>
      <span class="q-pct mono">${Math.round(pct)}%</span>
      ${compact ? "" : `<span class="q-reset">${esc(untilText(w.resetsAt))}</span>`}
    </div>`;
  }).join("") + more;
}

function aiCard(q, compact) {
  const st = q.status === "stale" ? `<span class="q-stale" title="${esc(q.message || "")}">上次数据 · ${ago(q.at)}</span>` : "";
  const ringed = !compact && q.windows?.length ? ringWindows(q) : [];
  const bars = q.windows?.length
    ? quotaBars(q, compact, ringed)
    : `<div class="q-msg ${q.status === "error" ? "bad" : ""}">${esc(q.message || "暂无数据")}</div>`;
  // big ring on the left (the share used of its outer window in the middle), the windows on the right
  const body = ringed.length
    ? `<div class="ai-body">${ringSvg(q, 88, ringed.length > 1 ? 7 : 9, `${Math.round(ringed[0].used)}%`)}<div class="ai-rows">${bars}</div></div>`
    : bars;
  return `<div class="ai-card ${q.status}">
    <div class="ai-head">${brandIcon(q.tool, 18)}<b>${esc(q.name)}</b>${q.plan ? `<span class="tag">${esc(q.plan)}</span>` : ""}${st}<span class="sp"></span>${!compact && q.source ? `<span class="q-src">${esc(q.source)}</span>` : ""}</div>
    ${body}
    ${!compact && q.status === "stale" && q.message ? `<div class="q-msg bad">${esc(q.message)}</div>` : ""}
  </div>`;
}

function aiTabHtml() {
  const live = agentsSectionHtml();
  if (!sget("ai.on", true)) return live + `<div class="empty">AI 额度已关闭（设置 → 额度与圆环）</div>`;
  const list = aiState.length ? aiState.map((q) => aiCard(q, false)).join("") : `<div class="empty">正在查询…</div>`;
  const last = aiState.reduce((m, q) => Math.max(m, q.at || 0), 0);
  const via = [...new Set(aiState.filter((q) => q.via).map((q) => q.via))];
  return `${live}<div class="viz">
    <div class="viz-head"><span class="viz-sel">订阅额度：用了多少、什么时候重置${last ? ` · 更新于 ${ago(last)}` : ""}${via.length ? ` · 经代理 127.0.0.1:${via.join("/")}` : ""}</span>
      <button class="ghost" data-do="ai-refresh">立即刷新</button></div>
    <div class="ai-grid">${list}</div>
    <div class="m-hint">用各工具在本机已登录的凭据向它自己的官方服务查询（Antigravity 读本机语言服务器），只读，凭据不保存、不发给其他地方；
      查询一律经代理软件的本地端口发出，找不到端口就先不查，不会直连；
      登录过期时请打开对应工具用一下，它会自己续期。每 ${sget("ai.interval", 10)} 分钟查一次（Claude 至少 10 分钟一次，点「立即刷新」也一样：官方接口限流很严，可能还和 Claude Code 自己共用）；结果会保存，重启 MicaBar 不会重新查。</div>
  </div>`;
}

// ---------- system monitor page ----------
// Built once, then updated in place from sysmon_sample (polled once a second while the page is
// visible), so bars can animate and nothing flickers.

let sysData = null;
let sysTimer = null;
const sysHist = { cpu: [], gpu: [] };
let corePeak = [];
let sysSort = "cpu";
try { sysSort = localStorage.getItem("mb-sys-sort") || "cpu"; } catch { /* ignore */ }
const GB = (b) => (b / 1073741824).toFixed(1);
const fmtMem = (b) => (b >= 1073741824 ? `${(b / 1073741824).toFixed(1)}G` : `${Math.round(b / 1048576)}M`);

function sysWanted() {
  return tab === "sys" && (MANAGER ? !document.hidden : expanded);
}
function ensureSysPoll() {
  if (sysWanted() && !sysTimer) sysTimer = setTimeout(sysPoll, 0);
}
function sysPoll() {
  if (!sysWanted()) { sysTimer = null; return; }
  invoke("sysmon_sample").then((d) => {
    sysData = d;
    const push = (arr, v) => { arr.push(v); if (arr.length > 60) arr.shift(); };
    if (d.cpu != null) push(sysHist.cpu, d.cpu);
    if (d.gpu != null) push(sysHist.gpu, d.gpu);
    // peak marks: hold the highest value, sinking slowly
    corePeak = d.cores.map((v, i) => Math.max(v, (corePeak[i] ?? 0) - 1.5));
    updateSys();
  }).catch(() => {}).finally(() => {
    // the first sample has no rates yet (they need two readings): take the second one soon
    sysTimer = sysWanted() ? setTimeout(sysPoll, sysData?.cores?.length ? 1000 : 250) : null;
  });
}
document.addEventListener("visibilitychange", ensureSysPoll);

// small line over a fixed 0..max scale; recessive, no axes (the number next to it is the reading)
function spark(values, cls, max = 100, w = 160, h = 28) {
  if (values.length < 2) return `<svg class="spark" width="${w}" height="${h}"></svg>`;
  const n = 60;
  const pts = values.map((v, i) => `${((i + n - values.length) / (n - 1)) * w},${h - 2 - (Math.min(v, max) / max) * (h - 4)}`).join(" ");
  return `<svg class="spark" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true"><polyline class="c-line ${cls}" points="${pts}" vector-effect="non-scaling-stroke"/></svg>`;
}

function sysPageHtml() {
  return `<div id="sysmon" class="sysmon">
    <div class="sm-main">
      <div class="sm-heads">
        <div class="sm-stat"><span class="sm-k">CPU</span><b class="mono" id="sm-cpu">–</b><span class="sm-u">%</span><span id="sm-cpu-line" class="sm-line"></span></div>
        <div class="sm-stat" id="sm-gpu-stat"><span class="sm-k">GPU</span><b class="mono" id="sm-gpu">–</b><span class="sm-u">%</span></div>
      </div>
      <div class="sm-bars" id="sm-bars"></div>
      <div class="sm-legend" id="sm-legend"></div>
      <div class="sm-meter"><span class="sm-k">内存</span><b class="mono" id="sm-mem">–</b><span class="meter"><i id="sm-mem-bar"></i></span></div>
      <div class="sm-meter" id="sm-vram-row"><span class="sm-k">显存</span><b class="mono" id="sm-vram">–</b><span class="meter"><i id="sm-vram-bar"></i></span></div>
      <div class="sm-net">
        <div><span class="sm-k">↓</span><b class="mono" id="sm-down">–</b><span id="sm-down-line" class="sm-line"></span></div>
        <div><span class="sm-k">↑</span><b class="mono" id="sm-up">–</b><span id="sm-up-line" class="sm-line"></span></div>
      </div>
    </div>
    <div class="sm-apps">
      <div class="viz-head">${segmented("sys-view", sysView, [["apps", "占用最多的程序"], ["startup", "开机自启"]])}${sysView === "apps"
        ? `<span id="sm-name" class="sm-name"></span><span class="sp"></span>${segmented("sys-sort", sysSort, [["cpu", "CPU"], ["mem", "内存"]])}`
        : `<span id="su-count" class="sm-name"></span><span class="sp"></span><button data-do="startup-refresh">刷新</button>`}</div>
      <div id="sm-list" class="sm-list ${sysView === "startup" ? "su-list" : ""}"><div class="empty">正在读取…</div></div>
      ${sysView === "startup" ? `<div class="m-hint">关掉 = 下次开机不自动启动它，和任务管理器「启动应用」是同一个开关，随时能再打开，不删除任何东西。计划任务和系统服务的自启不在这里。</div>` : ""}
    </div>
  </div>`;
}

// ---------- startup apps (Run keys + Startup folders, plus MicaBar's own task) ----------

let sysView = "apps";
try { sysView = localStorage.getItem("mb-sys-view") === "startup" ? "startup" : "apps"; } catch { /* ignore */ }
let startupItems = null;
let startupLoading = false;

function loadStartup() {
  if (startupLoading) return;
  startupLoading = true;
  if (!appInfo) refreshAppInfo(); // MicaBar's own row takes its icon from the exe path
  invoke("startup_list").then((l) => { startupItems = l; }).catch(() => { startupItems = startupItems || []; })
    .finally(() => { startupLoading = false; renderStartup(); });
}

function renderStartup() {
  const list = $("sm-list");
  if (!list || sysView !== "startup") return;
  if (!startupItems) { loadStartup(); return; }
  const self = sget("general.autostart", false);
  const n = startupItems.filter((i) => i.enabled).length + (self ? 1 : 0);
  const c = $("su-count");
  if (c) c.textContent = ` · ${n} 个会开机启动`;
  // rows hold still under the pointer (the switch just clicked already shows its new state)
  if (list.matches(":hover") && !list.querySelector(".empty")) return measurePanel();
  const row = (i) => `<div class="sm-app su-row ${i.enabled ? "" : "off"}">
      ${iconHtml({ name: i.name, path: i.path })}
      <span class="sm-app-name" title="${esc(i.command)}">${esc(i.name)}<span class="sub">${i.desc ? esc(i.desc) + " · " : ""}${esc(i.source)}${i.missing ? ` · <span class="su-miss">程序文件已不存在</span>` : ""}</span></span>
      <input type="checkbox" class="su-sw" data-su="${esc(i.id)}" ${i.enabled ? "checked" : ""} title="${i.enabled ? "开机自启：开" : "开机自启：关"}">
    </div>`;
  const mine = row({ id: "self", name: "MicaBar", desc: null, source: "计划任务 · 登录 10 秒后以管理员权限启动", command: "设置 → 常规 → 开机自启", path: appInfo?.exe || null, enabled: self, missing: false });
  list.innerHTML = mine + startupItems.map(row).join("");
  measurePanel();
}

document.addEventListener("change", async (e) => {
  const el = e.target.closest?.("[data-su]");
  if (!el) return;
  const id = el.dataset.su, on = el.checked;
  try {
    if (id === "self") {
      await invoke("settings_set", { patch: { general: { autostart: on } } });
    } else {
      await invoke("startup_set", { id, on });
      const it = startupItems?.find((i) => i.id === id);
      if (it) it.enabled = on;
    }
    el.closest(".su-row")?.classList.toggle("off", !on);
    toast(on ? "已设为开机自启" : "已关闭开机自启（随时能再打开）");
  } catch (err) {
    el.checked = !on;
    toast(String(err), true);
  }
  renderStartup();
});

function updateSys() {
  const d = sysData;
  if (!d || !$("sysmon")) return;
  const set = (id, v) => { const el = $(id); if (el && el.textContent !== v) el.textContent = v; };
  set("sm-cpu", d.cpu != null ? String(Math.round(d.cpu)) : "–");
  $("sm-cpu-line").innerHTML = spark(sysHist.cpu, "s1");
  $("sm-gpu-stat").hidden = d.gpu == null;
  set("sm-gpu", d.gpu != null ? String(Math.round(d.gpu)) : "–");
  // per-core bars (+ one for the GPU): performance and efficiency cores in two colours
  const classes = [...new Set(d.eff)].sort((a, b) => b - a);
  const hybrid = classes.length > 1;
  const kind = (i) => (hybrid && d.eff[i] !== classes[0] ? "e" : "p");
  const bars = $("sm-bars");
  const n = d.cores.length + (d.gpu != null ? 1 : 0);
  if (bars.childElementCount !== n) {
    bars.innerHTML = d.cores.map((_, i) => `<div class="sm-bar ${kind(i)}"><i></i><s></s></div>`).join("") +
      (d.gpu != null ? `<div class="sm-bar gpu"><i></i><span>GPU</span></div>` : "");
    $("sm-legend").innerHTML = hybrid
      ? `<span><i class="key sk-p"></i>性能核</span><span><i class="key sk-e"></i>能效核</span><span class="sm-dim">每根柱子一个逻辑核心，横线是最近的峰值</span>`
      : `<span class="sm-dim">每根柱子一个逻辑核心，横线是最近的峰值</span>`;
  }
  [...bars.children].forEach((b, i) => {
    const v = i < d.cores.length ? d.cores[i] : d.gpu;
    b.firstElementChild.style.height = `${Math.max(2, v)}%`;
    if (i < d.cores.length) {
      b.querySelector("s").style.bottom = `${Math.min(98, corePeak[i] ?? v)}%`;
      b.title = `${kind(i) === "e" ? "能效核" : hybrid ? "性能核" : "核心"} ${i}：${Math.round(v)}%`;
    } else {
      b.title = `GPU：${Math.round(v)}%${d.gpuName ? " · " + d.gpuName : ""}`;
    }
  });
  set("sm-name", d.cpuName ? ` · ${d.cpuName}` : "");
  set("sm-mem", d.memTotal ? `${GB(d.memUsed)} / ${GB(d.memTotal)} GB` : "–");
  $("sm-mem-bar").style.width = d.memTotal ? `${(d.memUsed / d.memTotal) * 100}%` : "0";
  const vr = d.vramTotal && d.vramUsed != null;
  $("sm-vram-row").hidden = !vr;
  if (vr) {
    set("sm-vram", `${GB(d.vramUsed)} / ${GB(d.vramTotal)} GB`);
    $("sm-vram-bar").style.width = `${Math.min(100, (d.vramUsed / d.vramTotal) * 100)}%`;
  }
  // network from the snapshots' history
  const tail = (a) => a.slice(-60);
  const maxNet = Math.max(1, ...tail(speedHist.down), ...tail(speedHist.up));
  set("sm-down", `${fmtRate(snap?.down || 0)}`);
  set("sm-up", `${fmtRate(snap?.up || 0)}`);
  $("sm-down-line").innerHTML = spark(tail(speedHist.down), "s1", maxNet);
  $("sm-up-line").innerHTML = spark(tail(speedHist.up), "s2", maxNet);
  // apps
  // the list holds still while the pointer is on it or a confirm card is open (rows would move
  // away under the click)
  const list = $("sm-list");
  if (sysView === "startup") {
    // the startup list changes only on a toggle or 刷新, not with every sample
    if (list.querySelector(".empty")) renderStartup();
    return measurePanel();
  }
  if (list.matches(":hover") || !$("menu").hidden) return measurePanel();
  const rows = [...d.apps].sort((a, b) => (sysSort === "mem" ? b.mem - a.mem : b.cpu - a.cpu || b.mem - a.mem)).slice(0, MANAGER ? 16 : 8);
  list.innerHTML = rows.map((a) => `<div class="sm-app">
      ${iconHtml(a)}<span class="sm-app-name">${esc(a.name)}${a.procs > 1 ? `<span class="sub">${a.procs} 个进程</span>` : ""}</span>
      <span class="mono sm-num ${sysSort === "cpu" ? "on" : ""}">${a.cpu.toFixed(1)}%</span><span class="mono sm-num ${sysSort === "mem" ? "on" : ""}">${fmtMem(a.mem)}</span>
      ${a.killable ? `<button class="sm-end" data-do="proc-end" data-name="${esc(a.name)}" data-n="${a.procs}" title="结束 ${esc(a.name)}">✕</button>` : `<span></span>`}
    </div>`).join("") || `<div class="empty">正在读取…</div>`;
  measurePanel();
}

// ---------- music page ----------

let musicKey = "";
function mediaPos(m) {
  const p = m.position + (m.playing && m.at ? (Date.now() - m.at) / 1000 : 0);
  return m.duration > 0 ? Math.min(m.duration, p) : p;
}
const mmss = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

function musicPageHtml() {
  const m = media;
  musicKey = m ? `${m.title}|${m.artist}|${m.playing}|${m.cover ? m.cover.length : 0}|${m.duration > 0}` : "";
  if (!sget("media.on", true)) return `<div class="empty">正在播放已关闭（设置 → 正在播放）</div>`;
  if (!m?.title) return `<div class="music empty-music"><div class="mu-cover none">♪</div><div class="mu-info"><div class="mu-title">没有正在播放的内容</div><div class="mu-artist">网易云、QQ 音乐、Spotify、浏览器里的视频等开始播放后会显示在这里</div></div></div>`;
  const cover = m.cover && sget("media.cover", true) ? m.cover : null;
  const pos = mediaPos(m);
  return `<div class="music" id="music">
    ${cover ? `<div class="mu-bg" style="background-image:url('${cover}')"></div>` : ""}
    ${cover ? `<img class="mu-cover" src="${cover}" alt="">` : `<div class="mu-cover none">♪</div>`}
    <div class="mu-info">
      <div class="mu-title">${esc(m.title)}</div>
      <div class="mu-artist">${esc([m.artist, m.album].filter(Boolean).join(" · "))}</div>
      <div class="mu-app">${esc(m.app || "")}</div>
      ${m.duration > 0 ? `<div class="mu-bar"><i id="mu-fill" style="width:${(pos / m.duration) * 100}%"></i></div>
        <div class="mu-time mono"><span id="mu-pos">${mmss(pos)}</span><span>${mmss(m.duration)}</span></div>` : ""}
      <div class="mu-ctl">
        <button data-do="media-prev" title="上一首">${ICON.prev}</button>
        <button class="main" data-do="media-toggle" title="播放 / 暂停">${m.playing ? ICON.pause : ICON.play}</button>
        <button data-do="media-next" title="下一首">${ICON.next}</button>
      </div>
      ${m.playing ? vizBars(12, true) : ""}
    </div>
  </div>`;
}

// progress between media events
setInterval(() => {
  if (tab !== "music" || !media?.duration || !$("mu-fill")) return;
  const pos = mediaPos(media);
  $("mu-fill").style.width = `${(pos / media.duration) * 100}%`;
  $("mu-pos").textContent = mmss(pos);
}, 1000);

// ---------- pages of the open island (icons at its top) ----------

const PAGE_DEFS = {
  net: ["网络", "M3 17l5-6 4 4 8-9"],
  sys: ["系统监控", "M6 20V12M10 20V6M14 20V10M18 20V4"],
  music: ["音乐", "M9 18V5l11-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z"],
  ai: ["AI 额度", "M12 3a9 9 0 1 0 9 9M12 7a5 5 0 1 0 5 5"],
  notes: ["通知", "M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0"],
};
const PAGE_TABS = new Set(["sys", "music", "ai", "notes"]);
let netTab = "conns";

function visiblePages() {
  const list = sget("panel.pages", null) || Object.keys(PAGE_DEFS).map((id) => ({ id, on: true }));
  const on = list.filter((p) => p.on && PAGE_DEFS[p.id]).map((p) => p.id);
  return on.length ? on : ["net"];
}
const curPage = () => (PAGE_TABS.has(tab) ? tab : "net");

function renderPages() {
  if (MANAGER) return;
  const pages = visiblePages();
  // the current page was switched off in the settings: go to the first one left
  if (!pages.includes(curPage())) tab = pages[0] === "net" ? netTab : pages[0];
  island.dataset.page = curPage();
  const html = pages.length > 1 ? pages.map((id) => {
    const [label, d] = PAGE_DEFS[id];
    const dot = id === "notes" && unreadCount() ? `<i class="pg-dot"></i>` : "";
    return `<button class="pg ${id === curPage() ? "active" : ""}" data-page="${id}" title="${label}" aria-label="${label}" role="tab" aria-selected="${id === curPage()}">
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>${dot}</button>`;
  }).join("") : "";
  if ($("pages").innerHTML !== html) $("pages").innerHTML = html;
}

function switchPage(id) {
  try { localStorage.setItem("mb-page", id); } catch { /* ignore */ }
  switchTab(id === "net" ? netTab : id);
}

if (!MANAGER) {
  $("pages").addEventListener("click", (e) => {
    const b = e.target.closest("[data-page]");
    if (b) switchPage(b.dataset.page);
  });
  // clock in the page bar
  setInterval(() => { if (expanded) $("page-clock").textContent = clockText(); }, 1000);
}

// ---------- overview (manager home) ----------

const svg = (d, fill) => `<svg width="14" height="14" viewBox="0 0 24 24" fill="${fill ? "currentColor" : "none"}" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="${d}"/></svg>`;
const ICON = {
  prev: svg("M6 5v14M19 5L9 12l10 7z", true),
  next: svg("M18 5v14M5 5l10 7-10 7z", true),
  play: svg("M7 4l13 8-13 8z", true),
  pause: svg("M7 5h3v14H7zM14 5h3v14h-3z", true),
};

// Overview AI block: one small table per tool, side by side across the page. Rows = quota pools
// (Antigravity: Gemini, Claude / GPT; others: one row), columns = windows (5 小时, 每周…), so the
// block grows by pools, not by windows, and many tools spread sideways instead of downwards.
function aiMini(q) {
  const st = q.status === "stale" ? `<span class="q-stale" title="${esc(q.message || "")}">上次数据 · ${ago(q.at)}</span>` : "";
  const head = `<div class="am-head"><b>${esc(q.name)}</b>${q.plan ? `<span class="tag">${esc(q.plan)}</span>` : ""}${st}</div>`;
  const ws = (q.windows || []).filter((w) => w && w.used != null);
  if (!ws.length) return `<div class="am">${head}<div class="q-msg ${q.status === "error" ? "bad" : ""}">${esc(q.message || "暂无数据")}</div></div>`;
  if (ws.length >= GROUP_FROM) return `<div class="am">${head}${quotaBars(q, true)}</div>`;
  const kindOf = (w) => (w.group && w.label.startsWith(w.group + " · ") ? w.label.slice(w.group.length + 3) : w.label);
  const kinds = [...new Set(ws.map(kindOf))];
  const pools = [...new Set(ws.map((w) => w.group || ""))];
  const named = pools.some((p) => p);
  const cols = `${named ? "minmax(0, 0.9fr) " : ""}repeat(${kinds.length}, minmax(0, 1fr))`;
  const cell = (w) => {
    if (!w) return `<span class="am-cell none">—</span>`;
    const pct = Math.max(0, Math.min(100, w.used));
    return `<span class="am-cell" title="${esc(`${w.label}：已用 ${Math.round(pct)}%，${untilText(w.resetsAt)}`)}"><span class="meter"><i class="${quotaClass(pct)}" style="width:${pct}%"></i></span><span class="q-pct mono">${Math.round(pct)}%</span></span>`;
  };
  const top = `${named ? "<span></span>" : ""}${kinds.map((k) => `<span class="am-kind">${esc(k)}</span>`).join("")}`;
  const rows = pools.map((p) => `${named ? `<span class="am-pool" title="${esc(p)}">${esc(p)}</span>` : ""}${kinds.map((k) => cell(ws.find((w) => (w.group || "") === p && kindOf(w) === k))).join("")}`).join("");
  return `<div class="am">${head}<div class="am-grid" style="grid-template-columns:${cols}">${top}${rows}</div></div>`;
}

function homeHtml() {
  if (!snap) return `<div class="empty">正在读取…</div>`;
  loadTraffic(false);
  loadLatency();
  const ex = exitState || {};
  const p = snap.proxy || {};
  const groups = ex.groups || [];
  const exitCards = [];
  if (p.detected && groups.length) {
    for (const g of groups) {
      const geo = [g.cc ? country(g.cc) : "", g.org || ""].filter(Boolean).join(" · ");
      exitCards.push(`<div class="hx tilt" data-tilt="5">
        <div class="hx-g">${decorate(g.group)}${g.isDefault ? `<span class="tag">默认</span>` : ""}</div>
        <div class="hx-ip mono">${g.ip ? `${flagImg(g.cc)}${esc(g.ip)}` : `<span class="crit">${g.error ? "检测失败" : "—"}</span>`}</div>
        <div class="hx-m">${esc(geo)}</div>
        ${g.node ? `<div class="hx-n">${decorate(g.node)}</div>` : ""}
      </div>`);
    }
  } else if (p.detected && ex.proxy) {
    const r = ex.proxy;
    exitCards.push(`<div class="hx tilt" data-tilt="5"><div class="hx-g">代理出口</div><div class="hx-ip mono">${r.ip ? `${flagImg(r.cc)}${esc(r.ip)}` : "—"}</div><div class="hx-m">${esc([r.cc ? country(r.cc) : "", r.org || ""].filter(Boolean).join(" · "))}</div>${r.node ? `<div class="hx-n">${decorate(r.node)}</div>` : ""}</div>`);
  }
  const d = ex.direct;
  exitCards.push(`<div class="hx direct tilt" data-tilt="5"><div class="hx-g">直连出口${p.detected ? "" : "（未检测到代理）"}</div>
    <div class="hx-ip mono">${d?.ip ? `${flagImg(d.cc)}${esc(d.ip)}` : "—"}</div>
    <div class="hx-m">${esc(d ? [d.cc ? country(d.cc) : "", d.org || ""].filter(Boolean).join(" · ") : "")}</div></div>`);
  const groupNote = p.detected && !groups.length
    ? ctrl().status === "active"
      ? `<div class="m-hint">分组出口要等覆写更新后才有：在「出口」页点「更新」</div>`
      : `<div class="m-hint">启用「出口控制」后，可以分别看到每个分组（AI、流媒体…）的出口 IP</div>`
    : "";

  const r = trafficData;
  const today = r?.days[r.days.length - 1]; // the traffic page may hold 7 or 30 days: the tile is today only
  const tp = today ? today.pu + today.pd : 0, td = today ? today.du + today.dd : 0;
  const L = snap.latency;
  const leaks = snap.guard?.unacked || 0;
  const tile = (label, value, note = "", go = "") => `<div class="tile tilt ${go ? "go" : ""}" ${go ? `data-tab="${go}"` : ""}><div class="tile-label">${label}</div><div class="tile-value">${value}</div>${note ? `<div class="tile-note">${note}</div>` : ""}</div>`;
  const tiles = `<div class="tiles home-tiles">
    ${tile("网速", `<span class="mono">${speedHtml(snap.up, snap.down)}</span>`, "", "speed")}
    ${tile("连接", `<span class="mono">${snap.connCount}</span>`, `${snap.apps.filter((a) => a.conns.length).length} 个程序`, "conns")}
    ${tile("今日流量", r ? fmtBytes(tp + td) : "—", r && tp + td ? `代理 ${Math.round((tp / (tp + td)) * 100)}% · 直连 ${Math.round((td / (tp + td)) * 100)}%` : "", "traffic")}
    ${tile("节点延迟", L?.at ? (L.ms != null ? `${L.ms} ms` : `<span class="crit">无响应</span>`) : "—", L?.node ? decorate(L.node) : "", "nodes")}
    ${tile("泄漏预警", leaks ? `<span class="crit">${leaks}</span>` : "0", leaks ? "点击查看" : "没有泄漏", "guard")}
  </div>`;

  const n = Math.min(120, speedHist.t.length);
  const chart = n >= 2
    ? Charts.line({
        id: "home-speed", width: chartW(), height: 130, xs: speedHist.t.slice(-n),
        series: [{ name: "下载", cls: "s1", values: speedHist.down.slice(-n) }, { name: "上传", cls: "s2", values: speedHist.up.slice(-n) }],
        fmtY: (v) => fmtRate(v), fmtTick: tickRate, fmtX: hms, endLabels: true, binary: true, area: true,
      })
    : "";

  const top = snap.apps.filter((a) => !a.isProxy && a.conns.length).slice(0, 8).map((a) => `<div class="top-row" data-tab="conns">
      ${iconHtml(a)}<span class="sp-name">${esc(a.name)}</span>
      <span class="route-tag ${a.viaProxy ? "proxy" : "direct"}">${a.viaProxy ? "代理" : "直连"}</span>
      <span class="app-count mono">${a.conns.length}</span>
      <span class="speed mono">${speedHtml(a.up, a.down)}</span></div>`).join("");

  const ai = sget("ai.on", true) && aiState.length
    ? `<div class="h-sec"><div class="h-head"><b>AI 额度</b><span class="sp"></span><button class="link" data-tab="ai">详情</button></div>
        ${(() => { const l = aiState.filter((q) => q.status !== "absent"); return l.length ? `<div class="am-list">${l.map(aiMini).join("")}</div>` : `<div class="q-msg">没找到已登录的 AI 工具</div>`; })()}</div>`
    : "";

  return `<div class="home">
    <div class="h-sec"><div class="h-head"><b>出口</b><span class="sp"></span>${ex.lastCheck ? `<span class="h-note">${ex.checking ? "检测中…" : "上次检测 " + ago(ex.lastCheck)}</span>` : ""}<button class="ghost" data-do="exit-refresh">重新检测</button></div>
      <div class="hx-grid">${exitCards.join("")}</div>${groupNote}</div>
    ${tiles}
    <div class="h-sec">${chart}</div>
    ${ai}
    <div class="h-sec"><div class="h-head"><b>正在联网的程序</b><span class="h-note">走代理的排在前面</span><span class="sp"></span><button class="link" data-tab="conns">全部连接</button></div>${top || `<div class="empty">没有连接</div>`}</div>
  </div>`;
}

// ---------- settings page ----------

const ROT_LABEL = {
  exits: "各分组的出口 IP", media: "正在播放的歌（播放时）", ai: "AI 额度", speed: "实时网速", latency: "节点延迟",
};
const PILL_ITEM_LABEL = {
  exits: "出口 IP（每个分组一个，并排显示）", speed: "实时网速",
  "ai-claude": "Claude 额度环", "ai-codex": "Codex 额度环", "ai-antigravity": "Antigravity 额度环", "ai-grok": "Grok 额度环",
  media: "正在播放的歌（播放时）", leaks: "泄漏数（有泄漏时才显示）", conns: "连接数", latency: "节点延迟",
  cpu: "CPU 占用", mem: "内存占用", clock: "时钟", exit: "主出口 IP（只显示一个）",
  rotate: "轮换区（下面勾选的内容轮流显示）",
  notes: "未读通知（有未读时才显示程序图标 / 数量）",
};

const HOURS = Array.from({ length: 24 }, (_, h) => [h, `${String(h).padStart(2, "0")}:00`]);
const SRC_LABEL = { toast: "系统通知", flash: "任务栏闪烁", popup: "程序弹窗", ai: "AI 钩子", test: "测试" };

function notifySettingsHtml() {
  // the tools' configs can change outside MicaBar: re-read when the page is built again later
  if (!notifyInfo || !notifyApps || Date.now() - notifyInfoAt > 10_000) loadNotifyInfo();
  const hooks = (notifyInfo?.hooks || []).map((h) => {
    const state = !h.found ? `没找到 ${esc(h.name)} 的配置文件夹` : h.installed ? "已接入：完成任务、等你确认时会通知" : h.conflict ? "已有别的 notify 设置，MicaBar 不覆盖" : "未接入";
    const btn = !h.found || (h.conflict && !h.installed) ? ""
      : h.installed ? `<button class="ghost" data-do="hook" data-tool="${h.tool}" data-on="0">移除</button>`
        : hookConfirm === h.tool ? `<button class="primary" data-do="hook" data-tool="${h.tool}" data-on="1">确认：修改 ${esc(h.path.replace(/^.*\\(\.[^\\]+\\[^\\]+)$/, "~\\$1"))}（先自动备份）</button>`
          : `<button class="ghost" data-do="hook" data-tool="${h.tool}" data-on="1">接入</button>`;
    return `<div class="s-row"><span class="s-label">${esc(h.name)}<span class="s-note">${state}</span></span>${btn}</div>`;
  }).join("");
  const apps = (notifyApps || []).filter((a) => a.source !== "test").map((a) => {
    const cfg = sget("notify.apps", {})?.[a.key] || {};
    const icon = a.iconUrl ? `<img class="app-icon" src="${a.iconUrl}" alt="">` : iconHtml({ path: a.icon, name: a.app });
    return `<div class="na-row">${icon}<span class="na-name" title="${esc(a.key)}">${esc(a.app)}</span><span class="na-src">${SRC_LABEL[a.source] || ""} · ${a.count} 条</span>
      <label><input type="checkbox" data-napp="${esc(a.key)}" data-f="island" ${cfg.island !== false ? "checked" : ""}> 岛上弹出</label>
      <label><input type="checkbox" data-napp="${esc(a.key)}" data-f="mute" ${cfg.mute ? "checked" : ""}> 静音</label>
      ${a.source === "toast" ? `<label title="关掉 Windows 自己右下角的弹窗，只在岛上显示（随时可以还原）"><input type="checkbox" data-ntake="${esc(a.key)}" ${a.takeover ? "checked" : ""}> 接管弹窗</label>`
        : a.source === "popup" ? `<label title="${esc(a.app)} 自己的右下角小窗一出现就藏起来，只在岛上显示"><input type="checkbox" data-napp="${esc(a.key)}" data-f="takeover" ${cfg.takeover ? "checked" : ""}> 接管弹窗</label>` : "<span></span>"}</div>`;
  }).join("");
  return `<div class="s-sec"><div class="s-title">通知<span class="s-note">其他程序的提醒在岛上弹出，收进「通知」页；内容只在内存里</span></div>
    ${sToggle("notify.on", "在岛上显示通知")}
    ${sSelect("notify.style", "　提示方式", [["icon", "胶囊右边一格，只显示程序图标"], ["inline", "胶囊里的内容暂时让开，显示一行消息"], ["card", "胶囊展开成卡片"]], "胶囊里有「未读通知」这一项时才显示图标（设置 → 收起外观 → 显示内容）")}
    ${sNumber("notify.duration", "　每次停留", 2, 30, 1, "秒", "鼠标放在通知上时一直停着；同时来几条会叠起来，放上去就展开")}
    ${sSelect("notify.privacy", "　显示内容", [["source", "不显示内容，只显示程序和发送人"], ["smart", "显示全文；全屏、录屏时只显示程序"], ["full", "始终显示全文"]], "岛上和通知页都按这个显示")}
    ${sToggle("notify.followDnd", "　跟随 Windows 勿扰", "Windows 开着勿扰 / 专注时，通知只收进通知页，不在岛上弹出")}
    ${sToggle("notify.quiet.on", "　免打扰时段")}
    ${sget("notify.quiet.on", false) ? `${sSelect("notify.quiet.from", "　　从", HOURS)}${sSelect("notify.quiet.to", "　　到", HOURS)}` : ""}
    ${sToggle("notify.toast", "　读取系统通知", "Telegram、邮件、浏览器等弹出 Windows 通知的程序，能显示发送人和内容")}
    ${sToggle("notify.flash", "　任务栏闪烁和程序自己的弹窗", "微信 4.x 来消息只闪任务栏，只能显示「微信 · N 条新消息」；Telegram 关了系统通知后用自己的小窗提醒，岛上能显示会话、发送人和内容（从它的聊天列表读）")}
    <div class="m-actions end"><button class="ghost" data-do="notify-test">发一条测试通知</button></div>
    <div class="s-sub">AI 完成提醒<span class="s-note">往工具自己的配置里加一个钩子，任务完成或等你批准时通知本机 127.0.0.1:${notifyInfo?.port ?? 47900}（带随机口令）</span></div>
    ${hooks || `<div class="s-note">正在读取…</div>`}
    <div class="s-sub">各个程序<span class="s-note">发过通知的程序会出现在这里</span></div>
    ${apps ? `<div class="na-list">${apps}</div>` : `<div class="s-note">还没有收到过通知</div>`}
  </div>`;
}
const PAGE_LABEL = Object.fromEntries(Object.entries(PAGE_DEFS).map(([k, v]) => [k, v[0]]));
const RING_MODES = [["both", "两圈：外圈短周期，内圈长周期"], ["first", "只看短周期（如 5 小时）"], ["second", "只看长周期（如每周）"], ["max", "用得最多的那个"]];

let monitorsCache = [];
let hotkeyRec = false;

function refreshMonitors() {
  invoke("monitors_list").then((m) => { monitorsCache = m || []; if (tab === "settings") renderList(true); }).catch(() => {});
}

// Hotkey recorder: the next key combination with a modifier becomes the hotkey.
document.addEventListener("keydown", async (e) => {
  if (!hotkeyRec) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.key === "Escape") {
    hotkeyRec = false;
    renderList(true);
    return;
  }
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter(Boolean);
  let key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
  if (key === "Backquote") key = "`";
  const combo = [...mods, key].join("+");
  const [, valid] = await invoke("hotkey_check", { candidate: combo });
  if (!mods.length || !valid) {
    toast(`${combo} 不行：要带 Ctrl / Alt / Shift / Win，且是字母、数字、F1~F24 等键`, true);
    return;
  }
  hotkeyRec = false;
  await setPath("interaction.hotkey", combo);
  setTimeout(async () => {
    const [ok] = await invoke("hotkey_check", { candidate: null });
    toast(ok ? `快捷键改成了 ${combo}` : `${combo} 被别的程序占用了，换一个吧`, !ok);
    renderList(true);
  }, 300);
}, true);

function setPath(path, value) {
  const patch = {};
  const keys = path.split(".");
  let o = patch;
  keys.slice(0, -1).forEach((k) => (o = o[k] = {}));
  o[keys[keys.length - 1]] = value;
  return invoke("settings_set", { patch }).catch((e) => toast(String(e), true));
}

// A label starting with full-width spaces ("　") is a sub-option of the row above: the whole row
// (label and its note, and the separator line) is indented one step per space, so the note
// lines up under the label instead of starting at the left edge.
function sRowOpen(label, note) {
  const m = /^　+/.exec(label);
  const lv = m ? Math.min(2, m[0].length) : 0;
  const text = m ? label.slice(m[0].length) : label;
  return `<label class="s-row${lv ? ` lv${lv}` : ""}"><span class="s-label">${text}${note ? `<span class="s-note">${note}</span>` : ""}</span>`;
}

function sToggle(path, label, note = "") {
  return `${sRowOpen(label, note)}
    <input type="checkbox" data-sp="${path}" ${sget(path, false) ? "checked" : ""}></label>`;
}

function sSelect(path, label, options, note = "") {
  const cur = String(sget(path, ""));
  return `${sRowOpen(label, note)}
    <select data-sp="${path}">${options.map(([v, t]) => `<option value="${esc(v)}" ${String(v) === cur ? "selected" : ""}>${esc(t)}</option>`).join("")}</select></label>`;
}

function sNumber(path, label, min, max, step, unit, note = "") {
  return `${sRowOpen(label, note)}
    <span class="s-num"><input type="number" data-sp="${path}" data-num="1" min="${min}" max="${max}" step="${step}" value="${esc(sget(path, min))}"><span class="s-unit">${unit}</span></span></label>`;
}

function sRange(path, label, min, max, step, unit, note = "") {
  const v = sget(path, min);
  return `${sRowOpen(label, note)}
    <span class="s-num"><input type="range" data-sp="${path}" data-num="1" min="${min}" max="${max}" step="${step}" value="${esc(v)}"><span class="mono s-val">${esc(v)}${unit}</span></span></label>`;
}

let groupOptions = null;
function groupPickerHtml() {
  if (!groupOptions) {
    invoke("exit_group_options").then((g) => { groupOptions = g || []; if (tab === "settings") renderList(true); }).catch(() => { groupOptions = []; });
    return `<div class="s-note">正在读取分组…</div>`;
  }
  if (!groupOptions.length) return `<div class="s-note">没有可选的分组：需要先启用出口控制（覆写里才有每个分组的专用端口）</div>`;
  const chosen = new Set(sget("exit.groups", []) || []);
  return `<div class="s-groups">${groupOptions.map((g) => `<label class="s-grp"><input type="checkbox" data-grp="${esc(g)}" ${chosen.has(g) ? "checked" : ""}> ${decorate(g)}</label>`).join("")}</div>`;
}

function sColor(path, label) {
  return `${sRowOpen(label, "")}<input type="color" data-sp="${path}" value="${esc(sget(path, "#000000"))}"></label>`;
}

let fontsCache = [];
function refreshFonts() {
  if (fontsCache.length) return;
  invoke("fonts_list").then((f) => { fontsCache = f || []; if (tab === "settings") renderList(true); }).catch(() => {});
}

// ---------- settings pages (manager window: the sidebar turns into these categories) ----------

const SET_NAV = [
  ["基础设置", [
    ["general", "常规", "M4 7h9M17 7h3M4 17h3M11 17h9M15 5v4M9 15v4"],
    ["collapsed", "收起外观", "M3 6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM9 4v1.5a1 1 0 0 0 1 1h4a1 1 0 0 0 1-1V4"],
    ["expanded", "展开外观", "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"],
    ["theme", "面板主题", "M12 3a9 9 0 1 0 0 18c1 0 1.5-.8 1.5-1.5 0-.8-.6-1.2-.6-2 0-.8.7-1.5 1.6-1.5H17a4 4 0 0 0 4-4c0-5-4-9-9-9zM7.5 11.5h0M10 7.5h0M14.5 7.5h0"],
  ]],
  ["网络", [["network", "出口与提示", "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9"]]],
  ["AI 用量", [["ai", "额度与圆环", "M12 3l2.5 5.5L20 11l-5.5 2.5L12 19l-2.5-5.5L4 11l5.5-2.5z"]]],
  ["音乐", [["music", "正在播放", "M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM21 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z"]]],
  ["通知", [["notify", "通知", "M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0"]]],
  ["其他", [["about", "关于", "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 16v-5M12 8h0"]]],
];
const SET_CATS = SET_NAV.flatMap(([, items]) => items.map((x) => x[0]));
let setCat = (() => { try { const c = localStorage.getItem("mb-set-cat"); return SET_CATS.includes(c) ? c : "general"; } catch { return "general"; } })();
let setBack = "home"; // the page the back button returns to

const navIcon = (d) => `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>`;

function settingsNavHtml() {
  return `<button class="tab nav back" data-setback title="回到${esc((NAV.find((n) => n[0] === setBack) || NAV[0])[1])}">${navIcon("M15 6l-6 6 6 6")}<span>返回</span></button>
    ${SET_NAV.map(([group, items]) => `<div class="nav-group">${group}</div>
      ${items.map(([id, label, d]) => `<button class="tab nav ${id === setCat ? "active" : ""}" data-setcat="${id}">${navIcon(d)}<span>${label}</span></button>`).join("")}`).join("")}`;
}

function setCategory(id) {
  if (!SET_CATS.includes(id)) return;
  setCat = id;
  try { localStorage.setItem("mb-set-cat", id); } catch { /* ignore */ }
  document.querySelectorAll("[data-setcat]").forEach((b) => b.classList.toggle("active", b.dataset.setcat === id));
  $("list").scrollTop = 0;
  Cascade.play($("list"));
  renderList(true);
}

// Entering / leaving the settings (desktop window): the sidebar items slide in one after the
// other from the side they come from, the page rises in. The class only stays for the length of
// the animation, so the settings page re-rendering after a change doesn't replay it.
function playIn(el, cls) {
  if (!el) return;
  [...el.children].forEach((c, i) => c.style.setProperty("--i", Math.min(i, 14)));
  el.classList.remove("nav-fwd", "nav-back", "page-in");
  void el.offsetWidth;
  el.classList.add(cls);
  clearTimeout(el._playT);
  el._playT = setTimeout(() => el.classList.remove(cls), 700);
}

const GRIP_SVG = `<svg width="10" height="16" viewBox="0 0 10 16" fill="currentColor"><circle cx="3" cy="3" r="1.4"/><circle cx="7" cy="3" r="1.4"/><circle cx="3" cy="8" r="1.4"/><circle cx="7" cy="8" r="1.4"/><circle cx="3" cy="13" r="1.4"/><circle cx="7" cy="13" r="1.4"/></svg>`;

// A list whose order matters: drag a row by its ⠿ handle (or Alt+↑ / ↓ on the handle); the switch
// shows / hides the item.
function sDragList(path, labels) {
  const list = (sget(path, []) || []).filter((i) => labels[i.id]);
  return `<div class="s-drag" data-dl="${path}">${list.map((i) => `<div class="s-drag-row ${i.on ? "" : "off"}" data-id="${esc(i.id)}">
      <button class="grip" data-grip title="拖动调整顺序（选中后也可以按 Alt+↑ / ↓）" aria-label="调整「${esc(labels[i.id])}」的顺序">${GRIP_SVG}</button>
      <span class="s-drag-name">${labels[i.id]}</span>
      <input type="checkbox" class="sw" data-ol="${path}" data-id="${esc(i.id)}" ${i.on ? "checked" : ""} aria-label="显示「${esc(labels[i.id])}」">
    </div>`).join("")}</div>`;
}

const CHECK_SVG = `<svg class="pick-check" width="18" height="18" viewBox="0 0 24 24"><circle cx="12" cy="12" r="11" fill="var(--blue)"/><path d="M7 12.5l3.2 3.2L17 9" fill="none" stroke="var(--on-accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

// collapsed look: a tiny screen with a window on it and the island at the top edge
function shapeCardsHtml() {
  const cur = sget("shape.style", "capsule");
  const card = (id, name) => `<button class="pick-card ${cur === id ? "on" : ""}" data-pick="shape.style" data-val="${id}" aria-pressed="${cur === id}">
      <div class="sc-mock tilt" data-tilt="9"><div class="sc-win"><i></i><i></i><i></i><i></i></div><div class="sc-shape ${id}"><b></b><b></b><b></b><b></b></div></div>
      <div class="pick-name"><span>${name}</span>${cur === id ? CHECK_SVG : ""}</div></button>`;
  return `<div class="pick-grid">${card("notch", "刘海")}${card("capsule", "胶囊")}${card("line", "指示条")}</div>`;
}

// a small pill in a theme's colours: flag, exit IP, speed, clock
const BASE_THEMES = {
  "glass-dark": { name: "黑色磨砂", bg: "#0c0c10", text: "#f2f2f5", dim: "#9a9aa6", green: "#34d399", amber: "#fbbf24" },
  "glass-light": { name: "白色磨砂", bg: "#f6f6f9", text: "#15151a", dim: "#4b4b57", green: "#059669", amber: "#b45309" },
};
function themePill(t) {
  return `<div class="tp-pill" style="--p-bg:${t.bg};--p-text:${t.text};--p-dim:${t.dim || t.text};--p-up:${t.amber || "#fbbf24"};--p-down:${t.green || "#34d399"}">
    <img class="tp-flag" src="flags/us.svg" alt=""><span class="mono">107.149.174.88</span>
    <span class="mono tp-sp"><b class="up">↑</b>12K <b class="down">↓</b>1.2M</span><span class="mono tp-clock">14:32</span></div>`;
}
function themeCardsHtml(ids) {
  const cur = sget("appearance.theme", "auto");
  const sys = lightQuery.matches ? "glass-light" : "glass-dark";
  return `<div class="pick-grid">${ids.map((id) => {
    let t, name;
    if (id === "auto") { t = BASE_THEMES[sys]; name = `跟随系统（现在是${lightQuery.matches ? "白" : "黑"}）`; }
    else if (BASE_THEMES[id]) { t = BASE_THEMES[id]; name = t.name; }
    else if (id === "custom") { const c = sget("appearance.custom", {}) || {}; t = { bg: c.bg || "#16161c", text: c.text || "#f2f2f5", dim: mixHex(c.text || "#f2f2f5", c.bg || "#16161c", 0.3) }; name = "自定义"; }
    else { t = PRESETS[id]; name = t.name; }
    return `<button class="pick-card theme ${cur === id ? "on" : ""}" data-pick="appearance.theme" data-val="${id}" aria-pressed="${cur === id}">
      <div class="tp-stage tilt" data-tilt="9">${themePill(t)}</div><div class="pick-name"><span>${esc(name)}</span>${cur === id ? CHECK_SVG : ""}</div></button>`;
  }).join("")}</div>`;
}

const sizeButtons = `<div class="m-actions end"><button class="ghost" data-do="size-reset">恢复默认大小</button><button class="primary" data-do="layout-edit">在屏幕上拖动调整</button></div>`;

const SET_PAGES = {
  general: () => `
  <div class="s-sec"><div class="s-title">启动与屏幕</div>
    ${sToggle("general.autostart", "开机自启", "登录 10 秒后以管理员权限启动（计划任务，不弹 UAC）")}
    ${sSelect("general.monitor", "显示在哪块屏幕", [["primary", "主屏幕"], ["cursor", "跟随鼠标所在的屏幕"], ...monitorsCache.map((m) => [m.name, `${m.name.replace("\\\\.\\", "")}（${m.w}×${m.h}${m.primary ? "，主屏" : ""}）`])])}
    ${sSelect("general.profile", "资源档位", [["saver", "省电"], ["balanced", "均衡"], ["smooth", "流畅"]], "影响节点测速等后台任务的频率")}
  </div>
  <div class="s-sec"><div class="s-title">交互</div>
    <div class="s-row"><span class="s-label">快捷键（展开 / 收起）${snap && !snap.hotkeyOk ? `<span class="s-note bad">没注册成功：可能被别的程序占用了</span>` : ""}</span>
      <button class="hk ${hotkeyRec ? "rec" : ""}" data-do="hotkey-rec">${hotkeyRec ? "请按下新的组合键…（Esc 取消）" : esc(sget("interaction.hotkey", "Ctrl+Alt+N"))}</button></div>
    ${sSelect("interaction.expand", "怎么展开", [["hover", "鼠标停留（收起时不挡点击）"], ["click", "点击（收起时胶囊会挡住下面的点击）"]])}
    ${sToggle("interaction.peek", "　鼠标停在胶囊上时变透明", "能看见被挡住的标签页等；点一下下面的东西就不会展开")}
    ${sNumber("interaction.hoverDelay", "　鼠标停留多久展开", 0, 2000, 50, "ms")}
    ${sNumber("interaction.holdMs", "　长按多久抓起岛", 150, 1000, 10, "ms", "按住面板或胶囊这么久就能拖着甩到左右，往屏幕中间拉开还能拖到别的边；太短的话普通点击也可能被当成长按")}
    ${sToggle("interaction.passthrough", "穿透模式", "鼠标完全不触发，点击穿透到下面的窗口；只能用快捷键打开")}
    ${sSelect("interaction.fullscreen", "全屏程序（游戏、视频）在前台时", [["pill", "显示胶囊，不自动展开"], ["hide", "完全隐藏"]])}
  </div>`,

  collapsed: () => `
  <div class="s-sec"><div class="s-title">收起样式</div>
    ${shapeCardsHtml()}
    ${sSelect("shape.dock", "停靠位置", [["top", "屏幕上边"], ["bottom", "屏幕下边（任务栏上方）"], ["left", "屏幕左边"], ["right", "屏幕右边"]], "左右两边时，胶囊会变成竖着的刘海；也可以长按岛往屏幕中间拉开，拖到另一条边松手")}
    ${sSelect("shape.anchor", "在这条边上的位置", [["start", "靠左（停靠左右两边时为靠上）"], ["center", "居中"], ["end", "靠右（停靠左右两边时为靠下）"]], "也可以长按岛把它甩过去：展开后长按面板，或按住 Alt 长按收起的胶囊")}
    ${sToggle("shape.lineWhenMaximized", "前台窗口最大化时缩成指示条", "和岛在同一块屏幕上的最大化窗口在前台时，胶囊 / 刘海临时变成指示条；切到别的窗口就变回来。每次启动后第一次变的时候，胶囊上会先提示一下")}
  </div>
  <div class="s-sec"><div class="s-title">显示内容<span class="s-note">拖动 ⠿ 调整顺序；宽度放不下时，排在后面的几项和轮换区合成最后一格轮流显示（指示条只显示状态色）</span></div>
    ${sDragList("pill.items", PILL_ITEM_LABEL)}
    ${sSelect("pill.clockStyle", "时钟样式", [["simple", "简洁"], ["digital", "数码管"], ["serif", "衬线"]])}
    ${sToggle("pill.clockSeconds", "时钟显示秒")}
  </div>
  <div class="s-sec"><div class="s-title">轮换区<span class="s-note">胶囊里的「轮换区」依次显示这些内容；没有数据的、胶囊里已经单独显示的自动跳过，不会重复</span></div>
    ${sDragList("rotate.items", ROT_LABEL)}
    ${sNumber("rotate.interval", "每项显示", 2, 60, 1, "秒")}
  </div>
  <div class="s-sec"><div class="s-title">大小与透明度<span class="s-note">也可以在屏幕上直接拖动边缘调整</span></div>
    ${sRange("size.pillH", "胶囊高度", 28, 64, 2, "px")}
    ${sRange("size.pillW", "胶囊宽度", 0, 1400, 10, "px", "0 = 跟着内容自动变宽")}
    ${sRange("size.pillMaxW", "　自动宽度最多", 200, 1400, 10, "px")}
    ${sRange("appearance.pillOpacity", "收起时胶囊的不透明度", 10, 100, 5, "%", "整颗胶囊连文字一起变淡，不那么显眼；展开后总是完全不透明")}
    ${sizeButtons}
  </div>`,

  expanded: () => `
  <div class="s-sec"><div class="s-title">展开后的页面<span class="s-note">岛顶部的图标切换；拖动 ⠿ 调整顺序，会记住上次停在哪一页</span></div>
    ${sDragList("panel.pages", PAGE_LABEL)}
  </div>
  <div class="s-sec"><div class="s-title">网络页的页签<span class="s-note">拖动 ⠿ 调整顺序</span></div>
    ${sDragList("panel.tabs", Object.fromEntries(Object.entries(TAB_DEFS).map(([k, v]) => [k, v.label])))}
  </div>
  <div class="s-sec"><div class="s-title">面板大小</div>
    ${sRange("size.panelW", "展开面板宽度", 480, 1100, 10, "px")}
    ${sRange("size.panelH", "展开面板最大高度", 360, 900, 10, "px", "内容少时会矮一些")}
    ${sSelect("appearance.scale", "界面缩放", [[90, "90%"], [100, "100%"], [110, "110%"], [125, "125%"], [150, "150%"]])}
    ${sizeButtons}
  </div>`,

  theme: () => `
  <div class="s-sec"><div class="s-title">基础</div>
    ${themeCardsHtml(["auto", "glass-dark", "glass-light"])}
  </div>
  <div class="s-sec"><div class="s-title">主题扩展</div>
    ${themeCardsHtml([...Object.keys(PRESETS), "custom"])}
    ${sget("appearance.theme") === "custom" ? `
      ${sSelect("appearance.custom.base", "基调", [["dark", "深色"], ["light", "浅色"]])}
      ${sColor("appearance.custom.bg", "底色")}
      ${sColor("appearance.custom.text", "文字颜色")}
      ${sColor("appearance.custom.accent", "强调色")}` : ""}
  </div>
  <div class="s-sec"><div class="s-title">磨砂与文字</div>
    ${sToggle("appearance.glass", "实时磨砂", "关掉后用纯色背景，同时停止截取屏幕（更省资源）")}
    ${sRange("appearance.opacity", "背景不透明度", 20, 100, 5, "", "50 = 默认；往右更不透明，往左更通透")}
    ${sSelect("appearance.legible", "背景亮度限制", [["normal", "标准（默认）"], ["strong", "强"], ["off", "关"]], "白磨砂下把背后的深色区域提亮、黑磨砂下把亮色区域压暗，背景一半亮一半暗时文字也看得清")}
    <label class="s-row"><span class="s-label">字体<span class="s-note">留空 = 系统默认</span></span>
      <span class="s-num"><input class="s-text" list="font-list" data-sp="appearance.font" value="${esc(sget("appearance.font", ""))}" placeholder="例如 微软雅黑" spellcheck="false"></span></label>
    <datalist id="font-list">${fontsCache.map((f) => `<option value="${esc(f)}">`).join("")}</datalist>
  </div>
  <div class="s-sec"><div class="s-title">动效</div>
    ${sSelect("appearance.motion", "界面动效", [["auto", "跟随 Windows（设置 → 辅助功能 → 动画效果）"], ["on", "总是打开"], ["off", "关闭"]], "胶囊展开变形、卡片倾斜、按钮回弹、数字滚动、列表依次出现这些动画")}
  </div>`,

  network: () => `
  <div class="s-sec"><div class="s-title">分组出口<span class="s-note">每个分组单独测一次出口 IP（经覆写里只监听本机的专用端口）</span></div>
    ${sSelect("exit.mode", "显示哪些分组", [["auto", "自动：每个不同的出口节点测一次（兜底分组和有流量的分组优先）"], ["manual", "手动勾选"]])}
    ${sNumber("exit.max", "　最多测", 1, 12, 1, "个出口", "用同一个节点的分组共用一次检测，不占名额")}
    ${sget("exit.mode") === "manual" ? groupPickerHtml() : ""}
  </div>
  <div class="s-sec"><div class="s-title">短暂提示<span class="s-note">发生时在胶囊上显示 5 秒，同类 1 分钟内只提示一次</span></div>
    ${sToggle("flash.exitChange", "出口 IP 变了")}
    ${sToggle("flash.nodeDown", "节点无响应")}
    ${sToggle("flash.latencyHigh", "节点延迟变高")}
    ${sNumber("flash.latencyMs", "　延迟超过", 100, 5000, 50, "ms")}
    ${sToggle("flash.speedSpike", "网速突增")}
    ${sNumber("flash.speedMBps", "　突增到超过", 1, 1000, 1, "MB/s")}
  </div>
  <div class="s-sec"><div class="s-title">断网诊断<span class="s-note">后台每 10 秒轻轻探一次，连着两次不通就从网卡一路查到节点</span></div>
    ${sToggle("diag.on", "自动发现断网", "胶囊最前面显示哪一环断了（网卡 / 路由器 / DNS / 宽带 / 代理软件 / 节点），点开有修复按钮；系统代理被别的软件关掉或改掉也会提醒")}
  </div>
  <div class="s-sec"><div class="s-title">新程序联网<span class="s-note">程序第一次连上公网时提醒，可以一键阻止</span></div>
    ${sToggle("firstNet.on", "提醒第一次联网的程序", "第一次开启时先花 10 分钟熟悉常用程序，不会一下子弹一堆；程序更新换了版本号目录也认得出来")}
    ${sToggle("firstNet.system", "　也提醒 Windows 自带的程序", "C:\\Windows 下的系统组件，数量多，一般不用开")}
  </div>`,

  ai: () => `
  <div class="s-sec"><div class="s-title">AI 额度<span class="s-note">用本机已登录的凭据向各家官方查询，只读，不保存、不外传</span></div>
    ${sToggle("ai.live", "正在跑的 AI 任务", "Claude Code 干活时胶囊最前面显示项目名和用时，等你批准时变成橙色；点一下切到它的终端。需要在「通知」里接好 Claude Code 钩子")}
    ${sToggle("ai.on", "显示 AI 额度")}
    ${sToggle("ai.claude", "　Claude Code")}
    ${sToggle("ai.codex", "　Codex", "需要用 ChatGPT 账号登录的 Codex")}
    ${sToggle("ai.antigravity", "　Antigravity", "Antigravity 打开着时才能读到（只问本机的语言服务器）")}
    ${sToggle("ai.grok", "　Grok", "需要用 grok CLI 登录过")}
    ${sNumber("ai.interval", "　每隔多久查一次", 1, 120, 1, "分钟", "Claude 最少 10 分钟一次，设得更短也按 10 分钟")}
    ${sNumber("ai.proxyPort", "　走哪个代理端口", 0, 65535, 1, "", "0 = 自动用代理软件的端口（Clash 的 mixed/http 端口或系统代理）。额度只经代理查询，找不到端口就不查，绝不直连；Antigravity 只问本机，不出网")}
  </div>
  <div class="s-sec"><div class="s-title">额度环<span class="s-note">胶囊和「AI 额度」页里的圆环</span></div>
    ${sSelect("ai.ring.mode.claude", "Claude 显示", RING_MODES)}
    ${sSelect("ai.ring.mode.codex", "Codex 显示", RING_MODES)}
    ${sSelect("ai.ring.mode.antigravity", "Antigravity 显示", RING_MODES, "两组模型（Gemini、Claude / GPT）各有 5 小时和每周额度，环显示用得多的那一组")}
    ${sSelect("ai.ring.mode.grok", "Grok 显示", RING_MODES)}
    ${sNumber("ai.ring.warn", "用到多少变成提醒色", 1, 99, 1, "%")}
    ${sNumber("ai.ring.crit", "用到多少变成警告色", 1, 100, 1, "%")}
    ${sColor("ai.ring.colors.ok", "　正常")}
    ${sColor("ai.ring.colors.warn", "　提醒")}
    ${sColor("ai.ring.colors.crit", "　警告")}
    <div class="m-actions end"><button class="ghost" data-do="ring-reset">恢复默认颜色和阈值</button></div>
  </div>`,

  music: () => `
  <div class="s-sec"><div class="s-title">正在播放</div>
    ${sToggle("media.on", "显示正在播放的歌", "读取 Windows 媒体控制（网易云、QQ 音乐、Spotify、浏览器等）")}
    ${sToggle("media.cover", "　显示封面")}
    ${sSelect("media.pillStyle", "　胶囊里的歌显示成", [["viz", "封面 + 随音乐跳动的线条"], ["title", "封面 + 歌名和歌手"]], "跳动的线条读的是正在播放的声音大小（只在本机算音量，不录音、不保存）")}
  </div>`,

  notify: () => notifySettingsHtml(),

  about: () => `
  <div class="s-sec"><div class="s-title">关于</div>
    <div class="s-row"><span class="s-label">MicaBar ${esc(appInfo?.version || "")}<span class="s-note">${appInfo ? (appInfo.installed ? "安装版" : "便携版（不能自动更新）") : ""}</span></span>
      <button class="ghost" data-do="update-check">检查更新</button></div>
    ${updateInfo?.available ? updateCardHtml() : ""}
    ${sToggle("general.autoUpdate", "自动检查更新", "每天检查一次 GitHub 上的新版本，有新版本时在胶囊上提示")}
    <div class="s-row"><span class="s-label">日志与数据<span class="s-note">${esc(appInfo?.logDir || "")}</span></span>
      <button class="ghost" data-do="open-logs">打开目录</button></div>
    <div class="s-row"><span class="s-label">新手教程<span class="s-note">在岛上重新走一遍第一次启动时的引导</span></span>
      <button class="ghost" data-do="tour-restart">重新看教程</button></div>
  </div>
  <div class="s-sec"><div class="s-title">作者与仓库</div>
    <div class="s-row"><span class="s-label">作者<span class="s-note">${AUTHOR}</span></span>
      <button class="ghost" data-do="open-author">GitHub 主页</button></div>
    <div class="s-row"><span class="s-label">源代码<span class="s-note mono">${REPO.replace("https://", "")}</span></span>
      <span><button class="ghost" data-do="open-issues">反馈问题</button> <button class="ghost" data-do="open-home">打开仓库</button></span></div>
    <div class="s-row"><span class="s-label">许可<span class="s-note">MIT 开源许可：可以自由使用、修改和分发，需保留版权声明</span></span></div>
  </div>
  <div class="s-sec"><div class="s-title">免责声明</div>
    <div class="legal">${DISCLAIMER.map((p) => `<p>${p}</p>`).join("")}</div>
  </div>
  <div class="s-sec"><div class="s-title">恢复默认</div>
    <div class="s-row"><span class="s-label">所有设置<span class="s-note">这几页里的所有设置回到第一次打开时的样子</span></span>
      <button class="ghost" data-do="settings-reset">恢复默认设置</button></div>
  </div>`,
};

function settingsHtml() {
  if (!S) return `<div class="empty">正在读取设置…</div>`;
  const label = SET_NAV.flatMap(([, items]) => items).find((x) => x[0] === setCat)?.[1] || "设置";
  return `<div class="set-page"><h1 class="set-h">${label}</h1>${(SET_PAGES[setCat] || SET_PAGES.general)()}</div>`;
}

let appInfo = null;
let updateInfo = null;

// "MicaBar 0.2.2：a；b；c" -> ["a", "b", "c"]
function updateNotes(text) {
  const body = String(text || "").replace(/^\s*MicaBar\s*[\d.]+\s*[:：]\s*/, "");
  return body.split(/[；;\n]+/).map((s) => s.trim().replace(/[。.]$/, "")).filter(Boolean);
}

function updateCardHtml() {
  const u = updateInfo;
  const notes = updateNotes(u.notes);
  const installed = u.installed;
  return `<div class="upd">
    <div class="upd-head">
      <span class="upd-badge">新版本</span>
      <span class="upd-ver"><b>MicaBar ${esc(u.version)}</b><span class="upd-cur">当前 ${esc(appInfo?.version || u.current || "")}</span></span>
      <span class="upd-btns">
        ${!installed ? `<button class="primary" data-do="open-releases">去下载</button>`
          : updProgress ? `<button class="primary" disabled>${updProgress.phase === "install" ? "正在安装…" : "下载中…"}</button>`
          : `<button class="primary" data-do="update-install">${updFailed ? "重试" : "下载并更新"}</button>`}
      </span>
    </div>
    ${updProgress ? updProgressHtml() : ""}
    ${notes.length ? `<ul class="upd-notes">${notes.map((n) => `<li>${esc(n)}</li>`).join("")}</ul>` : ""}
    <div class="upd-foot">${installed
      ? "下载后会校验签名并自动安装，MicaBar 会重启一次"
      : `便携版不能自动更新：下载新的 <span class="mono">MicaBar_${esc(u.version)}_portable.exe</span> 换掉现在这个即可，设置和记录都会保留；也可以改用安装版，以后自动更新`}</div>
  </div>`;
}
// download / install progress of the update, from the backend's "update-progress" events
let updProgress = null;
let updFailed = false;
const fmtMB = (b) => (b / 1048576).toFixed(1);
function updProgressParts() {
  const p = updProgress;
  if (p.phase === "install") return { pct: 100, text: "已下载并校验签名，正在安装；MicaBar 会关掉再自己打开（可能弹一次 UAC）" };
  const pct = p.total ? Math.min(100, Math.round((p.got / p.total) * 100)) : null;
  return { pct, text: p.total ? `下载中 ${pct}% · ${fmtMB(p.got)} / ${fmtMB(p.total)} MB` : `下载中 · ${fmtMB(p.got || 0)} MB` };
}
function updProgressHtml() {
  const { pct, text } = updProgressParts();
  return `<div class="upd-prog"><div class="upd-bar ${pct == null ? "busy" : ""}"><i style="width:${pct ?? 30}%"></i></div><div class="upd-ptext">${esc(text)}</div></div>`;
}
listen("update-progress", (e) => {
  const first = !updProgress || updProgress.phase !== e.payload.phase;
  updProgress = e.payload;
  const bar = document.querySelector(".upd-bar i");
  if (first || !bar) { renderList(true); return; }
  const { pct, text } = updProgressParts();
  bar.style.width = `${pct ?? 30}%`;
  const pt = document.querySelector(".upd-ptext");
  if (pt) pt.textContent = text;
});

function refreshAppInfo() {
  invoke("app_info").then((i) => {
    appInfo = i;
    if (tab === "settings") renderList(true);
    else if (tab === "sys" && sysView === "startup" && startupItems) { const l = $("sm-list"); if (l) l.innerHTML = `<div class="empty"></div>`; renderStartup(); }
  }).catch(() => {});
}
const REPO = "https://github.com/aileboeth/MicaBar";
const AUTHOR = "aileboeth";
const DISCLAIMER = [
  "MicaBar 是作者个人业余开发的免费开源软件（MIT 许可），按「现状」提供，不附带任何明示或暗示的担保，包括但不限于对适销性、特定用途适用性和不侵权的担保。",
  "它在本机读取网络连接、进程、系统通知、正在播放的媒体和 AI 工具的登录状态，这些数据只在本机使用，不会上传给作者；查询出口 IP、AI 额度、检查更新时会访问对应的服务（完整清单见仓库的 README）。「出口控制」「防泄漏」会修改代理软件的覆写配置、添加防火墙规则，并可能重启代理软件。",
  "因使用或无法使用本软件造成的任何直接或间接损失（包括但不限于数据丢失、断网、代理配置被改动、账号受限），作者不承担责任。请在遵守当地法律法规和各服务使用条款的前提下使用。",
  "MicaBar 与 Clash、mihomo、Anthropic、OpenAI、Google、xAI 等项目和公司没有任何关联，文中提到的名称和商标归各自的所有者。",
];

$("list").addEventListener("change", (e) => {
  const el = e.target;
  if (el.dataset.sp) {
    let v = el.type === "checkbox" ? el.checked : el.value;
    if (el.tagName === "SELECT" && /^\d+$/.test(v)) v = Number(v); // numeric options (scale)
    if (el.dataset.num) {
      v = Number(v);
      if (!Number.isFinite(v)) return;
      v = Math.min(Number(el.max), Math.max(Number(el.min), v));
    }
    setPath(el.dataset.sp, v);
  } else if (el.dataset.ol) {
    const list = (sget(el.dataset.ol, []) || []).map((i) => (i.id === el.dataset.id ? { ...i, on: el.checked } : i));
    setPath(el.dataset.ol, list);
  } else if (el.dataset.napp) {
    // app keys can contain dots (toast ids): patch the object directly
    invoke("settings_set", { patch: { notify: { apps: { [el.dataset.napp]: { [el.dataset.f]: el.checked } } } } }).catch((err) => toast(String(err), true));
  } else if (el.dataset.ntake) {
    invoke("notify_takeover", { key: el.dataset.ntake, on: el.checked })
      .then(() => { toast(el.checked ? "已关掉这个程序的 Windows 弹窗，只在岛上显示" : "已还原这个程序的 Windows 弹窗"); loadNotifyInfo(); })
      .catch((err) => { toast(String(err), true); loadNotifyInfo(); });
  } else if (el.dataset.grp != null) {
    const cur = new Set(sget("exit.groups", []) || []);
    if (el.checked) cur.add(el.dataset.grp);
    else cur.delete(el.dataset.grp);
    // keep the config order
    setPath("exit.groups", (groupOptions || []).filter((g) => cur.has(g)));
    invoke("refresh_exit").catch(() => {});
  }
});
// sliders show their value while being dragged
$("list").addEventListener("input", (e) => {
  const el = e.target;
  if (el.type === "range") {
    const out = el.parentElement.querySelector(".s-val");
    if (out) out.textContent = el.value + (out.textContent.match(/[^\d.]+$/)?.[0] || "");
  }
});

function moveItem(spec) {
  const list = [...(sget(spec.path, []) || [])];
  const i = list.findIndex((x) => x.id === spec.id);
  // skip over entries hidden from this page (e.g. tabs of later steps)
  let j = i + spec.dir;
  while (j >= 0 && j < list.length && spec.path === "panel.tabs" && !TAB_DEFS[list[j].id]) j += spec.dir;
  if (i < 0 || j < 0 || j >= list.length) return;
  [list[i], list[j]] = [list[j], list[i]];
  setPath(spec.path, list);
}

// ----- drag to reorder (settings lists) -----
// The row follows the pointer; the others slide out of the way (each move animated from where it
// was). Dropping saves the new order; entries this page doesn't show keep their place at the end.
let sortDrag = null;
let gripFocus = null; // {path, id}: focus that handle again after the page re-renders

$("list").addEventListener("pointerdown", (e) => {
  const g = e.target.closest("[data-grip]");
  if (!g || e.button !== 0) return;
  const row = g.closest(".s-drag-row");
  const box = row.parentElement;
  e.preventDefault();
  g.focus({ preventScroll: true });
  g.setPointerCapture(e.pointerId);
  const by = box.getBoundingClientRect().top;
  sortDrag = { pid: e.pointerId, row, box, grabY: e.clientY - by - row.offsetTop, moved: false, order: [...box.children].map((r) => r.dataset.id).join() };
  row.classList.add("dragging");
  box.classList.add("sorting");
});

$("list").addEventListener("pointermove", (e) => {
  if (!sortDrag || e.pointerId !== sortDrag.pid) return;
  const { row, box } = sortDrag;
  const y = e.clientY - box.getBoundingClientRect().top; // pointer, in the list's own coordinates
  const others = [...box.children].filter((r) => r !== row);
  const before = others.find((r) => y < r.offsetTop + r.offsetHeight / 2) || null;
  if (row.nextElementSibling !== before && !(before === null && box.lastElementChild === row)) {
    const was = new Map(others.map((r) => [r, r.offsetTop]));
    box.insertBefore(row, before);
    for (const r of others) {
      const dy = was.get(r) - r.offsetTop;
      if (!dy) continue;
      r.style.transition = "none";
      r.style.transform = `translateY(${dy}px)`;
      r.getBoundingClientRect();
      r.style.transition = "";
      r.style.transform = "";
    }
  }
  const top = Math.max(-6, Math.min(box.clientHeight - row.offsetHeight + 6, y - sortDrag.grabY));
  row.style.transform = `translateY(${top - row.offsetTop}px)`;
});

function endDrag(e) {
  if (!sortDrag || e.pointerId !== sortDrag.pid) return;
  const { row, box } = sortDrag;
  const path = box.dataset.dl;
  const ids = [...box.children].map((r) => r.dataset.id);
  row.style.transform = "";
  row.classList.remove("dragging");
  box.classList.remove("sorting");
  const changed = ids.join() !== sortDrag.order;
  sortDrag = null;
  if (!changed) return;
  const all = sget(path, []) || [];
  const shown = ids.map((id) => all.find((x) => x.id === id)).filter(Boolean);
  gripFocus = { path, id: row.dataset.id };
  setPath(path, [...shown, ...all.filter((x) => !ids.includes(x.id))]);
}
$("list").addEventListener("pointerup", endDrag);
$("list").addEventListener("pointercancel", endDrag);

// Alt+↑ / ↓ on a focused handle moves that row
$("list").addEventListener("keydown", (e) => {
  const g = e.target.closest?.("[data-grip]");
  if (!g || !e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
  e.preventDefault();
  const path = g.closest("[data-dl]").dataset.dl;
  const id = g.closest(".s-drag-row").dataset.id;
  gripFocus = { path, id };
  moveItem({ path, id, dir: e.key === "ArrowUp" ? -1 : 1 });
});

// picture cards (collapsed shape, theme)
$("list").addEventListener("click", (e) => {
  const p = e.target.closest("[data-pick]");
  if (p && sget(p.dataset.pick) !== p.dataset.val) setPath(p.dataset.pick, p.dataset.val);
});

// ---------- leak guard tab ----------

// programs that went online for the first time (loaded on demand, refreshed after actions)
let fnView = null;
let fnAt = 0;
function fnLoad(force) {
  if (!force && Date.now() - fnAt < 5000) return;
  fnAt = Date.now();
  invoke("firstnet_list").then((v) => { fnView = v; if (tab === "guard") renderList(true); }).catch(() => {});
}

function firstNetHtml() {
  fnLoad();
  const v = fnView;
  const on = sget("firstNet.on", true);
  const learning = v && v.learnUntil > Date.now();
  const rows = (v?.apps || []).map((k) => {
    const st = k.blocked ? "blocked" : k.allowed ? "allowed" : "new";
    const badge = { blocked: `<span class="fn-st blocked">已阻止</span>`, allowed: `<span class="fn-st allowed">已允许</span>`, new: `<span class="fn-st new">待处理</span>` }[st];
    const acts = k.blocked
      ? `<button data-do="fn" data-fn="unblock" data-path="${esc(k.path)}">恢复联网</button>`
      : `${st === "new" ? `<button data-do="fn" data-fn="allow" data-path="${esc(k.path)}">允许</button>` : ""}<button class="danger" data-do="fn" data-fn="block" data-path="${esc(k.path)}">阻止联网</button>`;
    return `<div class="fn-row ${st}">
      ${iconHtml({ path: k.path, name: k.name, exe: k.exe })}
      <div class="fn-main"><div class="fn-top"><b>${esc(k.name)}</b>${badge}<span class="fn-when">${ago(k.first)}</span></div>
        <div class="fn-sub" title="${esc(k.path)}">→ ${esc(k.dest || "")}<span class="fn-path">${esc(k.path)}</span></div></div>
      <div class="fn-acts">${acts}</div>
    </div>`;
  }).join("");
  const note = !on ? "已关闭"
    : learning ? `正在熟悉这台电脑上常用的程序（还剩 ${Math.ceil((v.learnUntil - Date.now()) / 60000)} 分钟），之后第一次联网的程序会弹出提醒`
    : "程序第一次连上公网时在岛上提醒，可以一键阻止（Windows 防火墙规则）";
  return `<div class="g-sec fn-sec">
    <div class="g-head"><b>新程序联网</b><span class="g-note">${note}</span><span class="sp"></span><button data-do="fn-toggle">${on ? "关闭" : "开启"}</button></div>
    ${rows || (on ? `<div class="empty fn-empty">${learning ? "学习中…" : "还没有新程序联网"}</div>` : "")}
  </div>`;
}

function guardTabHtml() {
  const g = snap.guard || {};
  const s = g.settings || {};
  const alerts = g.alerts || [];
  const ctlActive = ctrl().status === "active";
  const srcCount = (name) => (g.sources || []).find((x) => x[0] === name)?.[1] ?? 0;
  const ds = (o) => esc(JSON.stringify(o));

  const cards = alerts.length
    ? alerts.map((a) => {
        const cls = a.active ? (a.acked ? "acked" : "on") : "off";
        const what = a.host ? `${esc(a.host)}${a.port ? `<span class="port">:${a.port}</span>` : ""}` : `<span class="mono">${esc(a.ip || "")}</span>`;
        const kind = a.kind === "bypass" ? `<span class="a-kind bypass">绕过代理</span>` : `<span class="a-kind">规则漏网</span>`;
        const why = a.kind === "bypass"
          ? "没经过代理软件，直接从网卡连了出去（程序不走系统代理，或连接早于 TUN 建立）"
          : `经过了代理软件，但被分流规则判成直连${a.rule ? `（${esc(a.rule)}）` : ""}`;
        const acts = [];
        if (a.kind === "bypass" && a.path) {
          if (a.blocked) acts.push(`<span class="ok">✓ 已禁止 ${esc(a.exe)} 直连</span><button data-unblock="${esc(a.path)}">解除</button>`);
          else if (!g.tun) acts.push(`<button class="danger" data-block='${ds({ path: a.path, exe: a.exe })}'>禁止 ${esc(a.exe)} 直连</button>`);
        }
        if (a.kind === "rule" && !ctlActive) acts.push(`<button data-do="goto-rules">启用出口控制来自动纠正</button>`);
        if (a.host) acts.push(`<button data-exclude="${esc(a.host)}" title="以后这个域名直连也不预警">忽略这个域名</button>`);
        if (a.active && !a.acked) acts.push(`<button data-ack="${esc(a.key)}">知道了</button>`);
        return `<div class="alert-card ${cls}">
          <div class="a-top">${iconHtml({ path: a.path, name: a.app, exe: a.exe })}${kind}<span class="a-what"><b>${esc(a.app)}</b> → ${what}</span><span class="a-when">${a.active ? "进行中" : ago(a.last)}</span></div>
          <div class="a-sub">${why} · 命中「${esc(a.hit)}」${a.ip && a.host ? ` · <span class="mono">${esc(a.ip)}</span>` : ""}</div>
          ${acts.length ? `<div class="a-acts">${acts.join("")}</div>` : ""}
        </div>`;
      }).join("")
    : `<div class="empty">暂时没有泄漏。正在比对 ${g.total || 0} 条「应走代理」规则。</div>`;

  const opt = (key, label, note, extra = "") =>
    `<label class="g-opt"><input type="checkbox" data-opt="${key}" ${s[key] ? "checked" : ""}><span>${label}</span><span class="g-note">${note}</span>${extra}</label>`;
  const gfwNote = s.gfwError
    ? `<span class="g-err">${esc(s.gfwError)}</span>`
    : s.gfwUpdated ? `${srcCount("gfw")} 条 · 更新于 ${ago(s.gfwUpdated)}` : "还没下载";
  const forceNote = ctlActive
    ? "规则库里的网站要被判成直连时，改走 MicaBar 分组（不影响订阅已分到代理组的）"
    : "需要先在「出口」页启用出口控制";
  const imports = (s.imports || []).map((i) => `<label class="g-opt"><input type="checkbox" data-imp='${ds({ url: i.url })}' ${i.enabled ? "checked" : ""}><span class="g-note" title="${esc(i.url)}">${esc(i.url)}</span><span class="g-note" style="flex:none">${i.count} 条</span><button data-imprm="${esc(i.url)}">删除</button></label>`).join("");
  const kindLabel = { suffix: "", keyword: "关键词 ", cidr: "IP ", exclude: "排除 " };
  const chips = (s.custom || []).map((e) => `<span class="chip ${e.kind === "exclude" ? "exclude" : ""}"><span class="k">${kindLabel[e.kind] || ""}</span>${esc(e.value)}<button data-crm='${ds(e)}' title="删除">✕</button></span>`).join("");
  const blocked = (s.blocked || []).map((b) => `<div class="g-opt"><span>${esc(b.exe)}</span><span class="g-note" title="${esc(b.path)}">${esc(b.path)}</span><button data-unblock="${esc(b.path)}">解除</button></div>`).join("");

  return `<div class="g-sec">
      <div class="g-head"><b>泄漏预警</b><span class="sp"></span>
        ${g.unacked ? `<button data-do="ack-all">全部知道了</button>` : ""}
        ${alerts.some((a) => !a.active) ? `<button data-do="clear-alerts">清除已结束</button>` : ""}
      </div>${cards}
    </div>
    ${firstNetHtml()}
    <div class="g-sec">
      <div class="g-head"><b>应走代理的规则库</b><span>共 ${g.total || 0} 条</span></div>
      ${opt("builtin", "内置常见服务", `${srcCount("内置")} 个主域：Google、YouTube、GitHub、Telegram、X、OpenAI、Claude…`)}
      ${opt("gfw", "订阅 gfw 列表", gfwNote, `<button data-do="gfw-update">更新</button>`)}
      ${opt("forceProxy", "提前拦截：强制走代理", forceNote)}
      ${opt("notify", "系统通知", "同一程序 + 域名 10 分钟内只通知一次；全屏游戏时不弹")}
      <div class="g-head" style="margin-top:6px">导入规则列表（Clash / v2fly / 纯域名格式）</div>
      ${imports}
      <div class="g-row"><input id="g-import" placeholder="https://…/xxx.txt 或 .yaml" spellcheck="false"><button data-do="import">导入</button></div>
      <div class="g-head" style="margin-top:6px">自定义</div>
      <div class="chips">${chips || `<span class="g-note">还没有</span>`}</div>
      <div class="g-row"><input id="g-add" placeholder="域名 / IP 或 CIDR / keyword:关键词 / !域名（排除）" spellcheck="false"><button data-do="add">添加</button></div>
    </div>
    ${blocked ? `<div class="g-sec"><div class="g-head"><b>已禁止直连的程序</b><span>Windows 防火墙规则，只拦公网直连，本机代理端口不受影响</span></div>${blocked}</div>` : ""}`;
}

// While the cursor is over the list, rows keep their order (no re-sorting by live speed) and rows
// that disappeared stay as dimmed ghosts, so whatever the user is about to click doesn't move.
let listHover = false;
const appOrder = { order: null, last: new Map() };
const connOrders = new Map();

function stable(items, keyFn, st) {
  // also frozen while an exit menu is open (the cursor is on the menu, outside the list)
  if (!listHover && $("menu").hidden) {
    st.order = null;
    st.last = new Map(items.map((i) => [keyFn(i), i]));
    return items;
  }
  const cur = new Map(items.map((i) => [keyFn(i), i]));
  if (!st.order) st.order = items.map(keyFn);
  const known = new Set(st.order);
  for (const k of cur.keys()) if (!known.has(k)) st.order.push(k);
  const out = [];
  for (const k of st.order) {
    if (cur.has(k)) out.push(cur.get(k));
    else if (st.last.has(k)) out.push({ ...st.last.get(k), _gone: true });
  }
  for (const [k, v] of cur) st.last.set(k, v);
  return out;
}

const connKey = (c) => `${c.proto}|${c.localPort}|${c.remote}|${c.remotePort}`;

$("list").addEventListener("mouseenter", () => { listHover = true; });
$("list").addEventListener("mousemove", () => { listHover = true; });
$("list").addEventListener("mouseleave", () => {
  listHover = false;
  renderList();
});
// A click is a press and a release on the same element: rebuilding the rows in between (every
// snapshot does) swallows it. While a button is held over the list, the list is not rebuilt.
let listPress = false;
$("list").addEventListener("pointerdown", () => { listPress = true; });
const releaseList = () => {
  if (!listPress) return;
  listPress = false;
  setTimeout(() => renderList(), 60);
};
document.addEventListener("pointerup", releaseList);
document.addEventListener("pointercancel", releaseList);

function connHtml(app, c) {
  if (c._gone) {
    return `<div class="c gone"><div class="c-l1"><span class="c-proto mono">${c.proto}</span><span class="c-lp mono">:${c.localPort}</span><span class="c-arrow">→</span><span class="c-dst">${esc(c.host || c.remote || "")}</span><span class="c-spd">已断开</span></div></div>`;
  }
  const idx = refs.push([app, c]) - 1;
  const dst = c.host
    ? `${esc(c.host)}<span class="port">:${c.remotePort ?? ""}</span>`
    : c.remote
      ? `<span class="mono">${esc(c.remote)}</span><span class="port">:${c.remotePort ?? ""}</span>`
      : `<span class="port">—</span>`;
  const l2 = [];
  if (c.host && c.remote) l2.push(`<span class="mono">${esc(c.remote)}</span>`);
  if (c.cc) l2.push(`${flagImg(c.cc, "sm")} ${esc(country(c.cc))}`);
  if (c.org) l2.push(esc(c.org));
  if (c.node) {
    const route = `<span class="${routeClass(c.node)}">${decorate(c.node)}</span>${c.group ? " · " + decorate(c.group) : ""}`;
    l2.push(route);
  }
  if (c.rule && /micabar-guard/.test(c.rule)) l2.push(`<span class="ni" title="规则库里的网站本该被判成直连，守卫让它改走代理">守卫：强制走代理</span>`);
  else if (c.rule && /micabar-/.test(c.rule)) l2.push(`<span class="ni">MicaBar 规则</span>`);
  else if (c.rule) l2.push(esc(c.rule));
  if (c.viaProxyPort) l2.push("经本地代理端口");
  if (c.leak) l2.push(`<span class="leak" title="规则库里的「${esc(c.leak)}」本该走代理">⚠ 泄漏</span>`);
  if (c.bypass) l2.push(`<span class="st" title="代理软件在运行，但这条连接没经过它（程序不走系统代理，或连接早于 TUN 建立）">未经代理</span>`);
  if (c.state && c.state !== "ESTABLISHED" && c.state !== "ACTIVE") l2.push(`<span class="st">${esc(c.state)}</span>`);
  const sp = (c.up || c.down) ? `<span class="c-spd mono">${speedHtml(c.up, c.down)}</span>` : "";
  const act = !app.isProxy && ctrl().status !== "unavailable" && (c.host || c.remote)
    ? `<button class="c-act" data-act="${idx}" title="这条连接走哪个出口">⋯</button>` : "";
  return `<div class="c">
    <div class="c-l1"><span class="c-proto mono ${c.proto === "UDP" ? "udp" : ""}">${c.proto}</span><span class="c-lp mono">:${c.localPort}</span><span class="c-arrow">→</span><span class="c-dst">${dst}</span>${sp}${act}</div>
    ${l2.length ? `<div class="c-l2">${l2.join(" · ")}</div>` : ""}
  </div>`;
}

function renderList(force = false) {
  renderListNow(force);
  afterRender();
}

function renderListNow(force) {
  const list = $("list");
  if (listPress && !force) return;
  if (tab === "settings") {
    // the settings page only re-renders when settings change (not on every snapshot), so open
    // dropdowns and number fields are not destroyed while the user works with them
    if (!force || sortDrag) return;
    const scroll = list.scrollTop;
    list.innerHTML = `<div id="list-inner">${settingsHtml()}</div>`;
    list.scrollTop = scroll;
    if (gripFocus) {
      const r = [...list.querySelectorAll(`[data-dl="${gripFocus.path}"] .s-drag-row`)].find((x) => x.dataset.id === gripFocus.id);
      r?.querySelector("[data-grip]")?.focus({ preventScroll: true });
      gripFocus = null;
    }
    measurePanel();
    return;
  }
  if (tab === "sys" || tab === "music") {
    // built once and updated in place (sys) or rebuilt only when the song / state changes (music)
    const m = media;
    const key = m ? `${m.title}|${m.artist}|${m.playing}|${m.cover ? m.cover.length : 0}|${m.duration > 0}` : "";
    if (!force && (tab === "sys" ? $("sysmon") : list.querySelector(".music") && key === musicKey)) return;
    list.innerHTML = `<div id="list-inner">${tab === "sys" ? sysPageHtml() : musicPageHtml()}</div>`;
    if (tab === "sys") { updateSys(); renderStartup(); ensureSysPoll(); }
    updateViz();
    measurePanel();
    return;
  }
  if (!snap) {
    list.innerHTML = `<div class="empty">正在读取连接…</div>`;
    return;
  }
  // don't wipe what the user is typing into an input inside the list
  if (list.contains(document.activeElement) && ["INPUT", "SELECT"].includes(document.activeElement.tagName)) return;
  const rows = [];
  refs = [];
  if (tab === "speed" || tab === "traffic" || tab === "nodes" || tab === "home" || tab === "ai" || tab === "notes") {
    const scroll = list.scrollTop;
    const html = { speed: speedTabHtml, traffic: trafficTabHtml, nodes: nodesTabHtml, home: homeHtml, ai: aiTabHtml, notes: notesPageHtml }[tab]();
    list.innerHTML = `<div id="list-inner">${html}</div>`;
    list.scrollTop = scroll;
    Charts.restore(list);
    measurePanel();
    // the notes page was looked at: everything on it counts as seen (the pill icons go away)
    if (tab === "notes" && (MANAGER ? !document.hidden : expanded)) {
      const un = notes.filter((n) => !n.read).map((n) => n.id);
      if (un.length) setTimeout(() => { readNotes(un); renderPill(); if (!MANAGER) renderPages(); }, 1500);
    }
    return;
  }
  if (tab === "rules") {
    rows.push(rulesTabHtml());
  } else if (tab === "guard") {
    rows.push(guardTabHtml());
  } else if (tab === "conns") {
    const leaks = snap.guard?.unacked || 0;
    if (diag.problem && sget("diag.on", true)) {
      const dp = diag.problem;
      rows.push(`<div class="banner alert dg-banner ${dp.kind === "sysproxy" ? "warn" : ""}">${svgI(dp.kind === "sysproxy" ? DG_PATH.plug : DG_PATH.wifiOff, 15, 2.2)}<span><b>${esc(dp.problem || "网络异常")}</b> · ${esc(dp.verdict || "")}</span><button class="primary" data-do="diag-open">查看诊断</button></div>`);
    }
    if (leaks) rows.push(`<div class="banner alert">⚠ 检测到 ${leaks} 条代理泄漏：本该走代理的连接直连了。<button class="primary" data-do="goto-guard">查看</button></div>`);
    const b = bannerHtml();
    if (b) rows.push(b);
    const visible = snap.apps.filter((app) => app.conns.some((c) => matches(app, c)));
    for (const app of stable(visible, (a) => a.key, appOrder)) {
      if (!connOrders.has(app.key)) connOrders.set(app.key, { order: null, last: new Map() });
      const conns = app._gone ? [] : stable(app.conns.filter((c) => matches(app, c)), connKey, connOrders.get(app.key));
      if (app._gone) {
        rows.push(`<div class="app gone" data-key="${esc(app.key)}"><div class="app-head">${iconHtml(app)}<div class="app-name">${esc(app.name)}<span class="sub">连接已结束</span></div></div></div>`);
        continue;
      }
      const open = openApps.has(app.key) || (query && conns.length <= 8);
      const procs = app.pids.length > 1 ? `<span class="sub">${app.pids.length} 个进程</span>` : "";
      const tag = app.isProxy ? `<span class="tag">代理</span>` : "";
      let body = "";
      if (open) {
        const limit = showAll.has(app.key) ? conns.length : MAX_CONNS_PER_APP;
        body = `<div class="conns">${conns.slice(0, limit).map((c) => connHtml(app, c)).join("")}${
          conns.length > limit ? `<div class="more" data-more="${esc(app.key)}">还有 ${conns.length - limit} 条，点击显示全部</div>` : ""
        }</div>`;
      }
      rows.push(`<div class="app ${open ? "open" : ""}" data-key="${esc(app.key)}">
        <div class="app-head" tabindex="0" title="${esc(app.path || app.exe)}">
          ${iconHtml(app)}
          <div class="app-name">${esc(app.name)}${tag}${procs}</div>
          <div class="app-count mono">${conns.length}</div>
          <div class="speed mono">${snap.speedOk || app.up || app.down ? speedHtml(app.up, app.down) : ""}</div>
          ${exitChip(app)}
          <svg class="chev" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M9 6l6 6-6 6"/></svg>
        </div>${body}
      </div>`);
    }
  } else {
    for (const app of snap.apps) {
      const ports = app.listen.filter((p) => !query || matches(app) || p.toLowerCase().includes(query.toLowerCase()));
      if (!ports.length) continue;
      rows.push(`<div class="app open" data-key="${esc(app.key)}">
        <div class="app-head" tabindex="0" title="${esc(app.path || app.exe)}">
          ${iconHtml(app)}
          <div class="app-name">${esc(app.name)}${app.isProxy ? `<span class="tag">代理</span>` : ""}</div>
          <div class="app-count mono">${ports.length}</div>
        </div>
        <div class="ports">${ports.map((p) => {
          const pub = /\s(0\.0\.0\.0|::):/.test(p);
          return `<span class="port-chip mono ${pub ? "pub" : ""}" title="${pub ? "监听所有网卡（局域网可访问）" : ""}">${esc(p)}</span>`;
        }).join("")}</div>
      </div>`);
    }
  }
  const scroll = list.scrollTop;
  list.innerHTML = `<div id="list-inner">${rows.length ? rows.join("") : `<div class="empty">${query ? "没有匹配的结果" : "没有连接"}</div>`}</div>`;
  list.scrollTop = scroll;
  measurePanel();
}

function renderPanel() {
  if (!expanded) return;
  renderLiveBar();
  renderExit();
  renderProxyLine();
  const setCount = (id, v) => { const el = $("tab-" + id); if (el) el.textContent = v; };
  setCount("conns", snap?.connCount ?? 0);
  setCount("listen", snap?.listenCount ?? 0);
  setCount("rules", ctrl().rules.length);
  setCount("guard", snap?.guard?.unacked || 0);
  $("tab-guard")?.parentElement.classList.toggle("hot", !!snap?.guard?.unacked);
  if ($("tab-notes")) {
    const u = unreadCount();
    setCount("notes", u ? u : "");
    $("tab-notes").parentElement.classList.toggle("has", u > 0);
  }
  $("gear").classList.toggle("active", tab === "settings");
  $("total-speed").innerHTML = snap && (snap.speedOk || snap.up || snap.down) ? speedHtml(snap.up, snap.down) : "";
  renderList();
  const foot = [];
  if (snap) {
    if (!snap.speedOk) foot.push(`<span class="warn">每程序网速不可用：${esc(snap.speedError || "")}</span>`);
    if (snap.hiddenLoopback) foot.push(`已隐藏 ${snap.hiddenLoopback} 条本机回环连接`);
    if (!snap.geoSource) foot.push(`<span class="warn">没找到 IP 归属地库（country.mmdb / ASN.mmdb）</span>`);
  }
  $("foot").innerHTML = foot.map((x) => `<span>${x}</span>`).join("");
  measurePanel();
}

// ---------- events ----------

async function runAction(el) {
  const act = el.dataset.do;
  try {
    if (act === "diag-open") {
      openDiagSheet(true);
      return;
    } else if (act === "agent-focus") {
      await invoke("agents_focus", { id: el.dataset.id });
      return;
    } else if (act === "page-ai") {
      switchPage("ai");
      return;
    } else if (act === "fn") {
      el.disabled = true;
      const msg = await invoke("firstnet_action", { path: el.dataset.path, action: el.dataset.fn, note: null });
      if (msg) toast(msg);
      fnLoad(true);
      return;
    } else if (act === "fn-toggle") {
      await setPath("firstNet.on", !sget("firstNet.on", true));
      fnLoad(true);
      return;
    } else if (act === "enable") {
      // installing / updating the override restarts the proxy client: ask first
      const c = ctrl();
      const client = c.client || "Clash Party";
      const what = c.status === "available" && c.message ? "更新覆写" : c.status === "error" ? "重试" : "启用出口控制";
      const html = `<div class="m-title">${what}需要重启 ${esc(client)}</div>
        <div class="m-warn">MicaBar 会给 ${esc(client)} 写入一个全局覆写，然后把它关掉再打开一次：
          <ul><li><b>代理会断开约 3~5 秒</b>，正在进行的下载、通话、游戏可能会掉线</li>
          <li>${esc(client)} 的窗口可能会弹出来</li>
          <li>加载失败会自动还原成原来的设置</li></ul></div>
        <div class="m-actions"><button class="ghost" data-do="menu-close">取消</button><button class="primary" data-do="enable-go">现在重启并${what === "重试" ? "重试" : what.replace("启用出口控制", "启用")}</button></div>`;
      if (el.closest && el.closest("#menu")) {
        $("menu").innerHTML = html; // already inside a menu: replace it in place
      } else {
        openMenu(el, html);
      }
      return;
    } else if (act === "enable-go") {
      closeMenu();
      toast(`正在写入覆写并重启 ${ctrl().client || "Clash Party"}（代理会断开几秒）…`);
      await invoke("control_enable");
      toast("出口控制已启用");
    } else if (act === "menu-close") {
      closeMenu();
      return;
    } else if (act === "dismiss") {
      try { localStorage.setItem("ni-setup-dismissed", "1"); } catch { /* ignore */ }
    } else if (act === "disable") {
      if (!confirmDisable) {
        confirmDisable = true;
        setTimeout(() => { confirmDisable = false; renderList(); }, 4000);
      } else {
        confirmDisable = false;
        toast("正在移除覆写并重启 Clash Party…");
        await invoke("control_disable");
        toast("出口控制已停用（规则保留，重新启用即可恢复）");
      }
    } else if (act === "notify-test") {
      await invoke("notify_test");
      if (MANAGER || expanded) toast("测试通知已发出：收起面板时会在岛上弹出，也在通知页里");
      return;
    } else if (act === "hook") {
      // changes the tool's own config file: one more click to confirm
      const tool = el.dataset.tool;
      const on = el.dataset.on === "1";
      if (on && hookConfirm !== tool) {
        hookConfirm = tool;
        setTimeout(() => { if (hookConfirm === tool) { hookConfirm = null; renderList(true); } }, 6000);
        renderList(true);
        return;
      }
      hookConfirm = null;
      toast(await invoke("notify_hook", { tool, on }));
      loadNotifyInfo();
      return;
    } else if (act === "notes-clear") {
      notes = [];
      ncOpen.clear();
      await invoke("notify_dismiss", { ids: [] });
      hideNotes();
      afterNotesChange();
      return;
    } else if (act === "notes-read") {
      readNotes(notes.map((n) => n.id));
      afterNotesChange();
      return;
    } else if (act === "note-mute") {
      closeMenu();
      toggleAppMute(el.dataset.key);
      return;
    } else if (act === "note-clear") {
      closeMenu();
      const ids = notes.filter((n) => n.key === el.dataset.key).map((n) => n.id);
      notes = notes.filter((n) => !ids.includes(n.id));
      invoke("notify_dismiss", { ids }).catch(() => {});
      afterNotesChange();
      return;
    } else if (act === "update-check") {
      toast("正在检查更新…");
      updateInfo = await invoke("update_check");
      toast(updateInfo.available ? `有新版本 ${updateInfo.version}` : "已经是最新版本");
      renderList(true);
      return;
    } else if (act === "update-install") {
      updFailed = false;
      updProgress = { phase: "download", got: 0, total: null };
      renderList(true);
      try {
        await invoke("update_install");
      } catch (err) {
        updProgress = null;
        updFailed = true;
        renderList(true);
        throw err;
      }
      return;
    } else if (act === "proc-end") {
      const name = el.dataset.name, n = Number(el.dataset.n) || 1;
      openMenu(el, `<div class="m-title">结束「${esc(name)}」？</div>
        <div class="m-warn">会强制结束它的 ${n > 1 ? `全部 ${n} 个进程` : "进程"}，<b>没保存的内容会丢失</b>。Windows 自己的核心进程和 MicaBar 不会被结束。</div>
        <div class="m-actions"><button class="ghost" data-do="menu-close">取消</button><button class="danger" data-do="proc-end-go" data-name="${esc(name)}">结束</button></div>`);
      return;
    } else if (act === "startup-refresh") {
      startupItems = null;
      renderStartup();
      return;
    } else if (act === "proc-end-go") {
      closeMenu();
      const [ok, bad] = await invoke("sysmon_end", { name: el.dataset.name });
      toast(ok ? `已结束 ${el.dataset.name}（${ok} 个进程）${bad ? `，${bad} 个没结束掉（权限不够或已退出）` : ""}` : `没能结束 ${el.dataset.name}${bad ? "：权限不够" : "：它可能已经退出了"}`, !ok);
      return;
    } else if (act === "ring-reset") {
      await invoke("settings_set", { patch: { ai: { ring: { warn: 60, crit: 85, colors: { ok: "#34d399", warn: "#fbbf24", crit: "#f87171" } } } } });
      toast("额度环的颜色和阈值已恢复默认");
      return;
    } else if (act === "open-releases") {
      invoke("open_url", { url: REPO + "/releases/latest" });
      return;
    } else if (act === "open-home") {
      invoke("open_url", { url: REPO });
      return;
    } else if (act === "open-issues") {
      invoke("open_url", { url: REPO + "/issues" });
      return;
    } else if (act === "open-author") {
      invoke("open_url", { url: "https://github.com/" + AUTHOR });
      return;
    } else if (act === "tour-restart") {
      invoke("tour_restart").then(() => toast("教程在岛上开始了")).catch((err) => toast(String(err), true));
      return;
    } else if (act === "open-logs") {
      if (appInfo?.logDir) invoke("open_url", { url: appInfo.logDir });
      return;
    } else if (act === "lat-probe") {
      toast("测速中…");
      const l = await invoke("latency_probe");
      toast(l.ms != null ? `${l.ms} ms` : `无响应：${l.error || ""}`, l.ms == null);
    } else if (act === "hotkey-rec") {
      hotkeyRec = !hotkeyRec;
      renderList(true);
      return;
    } else if (act === "settings-reset") {
      await invoke("settings_reset");
      toast("已恢复默认设置");
      return;
    } else if (act === "size-reset") {
      await setPath("size", { ...SIZE_DEFAULTS });
      toast("已恢复默认大小");
      return;
    } else if (act === "layout-edit") {
      await invoke("layout_edit", { on: true });
      toast("到屏幕上的岛那里拖动边缘调整大小，调完点「完成」");
      return;
    } else if (act === "ai-refresh") {
      const waiting = (await invoke("ai_refresh")) || [];
      toast(waiting.length
        ? `正在查询；${waiting.map((w) => w.replace(" ", " 要到 ")).join("、")} 才能再查（官方接口限流很严，查太勤会被暂停）`
        : "正在查询 AI 额度…");
      return;
    } else if (act === "exit-refresh") {
      await invoke("refresh_exit");
      if (exitState) exitState.checking = true;
      toast("正在重新检测出口…");
    } else if (act === "media-prev" || act === "media-toggle" || act === "media-next") {
      await invoke("media_control", { action: { "media-prev": "previous", "media-toggle": "play-pause", "media-next": "next" }[act] });
      return;
    } else if (act === "open-manager") {
      await invoke("open_manager", { page: el.dataset.page || "home" });
      return;
    } else if (act === "goto-guard" || act === "goto-rules") {
      switchTab(act === "goto-guard" ? "guard" : "rules");
      return;
    } else if (act === "ack-all") {
      await invoke("alert_ack", { key: null });
    } else if (act === "clear-alerts") {
      await invoke("alert_clear");
    } else if (act === "gfw-update") {
      toast("正在下载 gfw 列表…");
      const n = await invoke("guard_update_gfw");
      toast(`gfw 列表已更新：${n} 条`);
    } else if (act === "import") {
      const url = $("g-import").value.trim();
      if (!url) return;
      toast("正在下载…");
      const n = await invoke("guard_import", { url });
      $("g-import").value = "";
      $("g-import").blur();
      toast(`已导入 ${n} 条`);
    } else if (act === "add") {
      const text = $("g-add").value.trim();
      if (!text) return;
      await invoke("guard_add", { text });
      $("g-add").value = "";
      $("g-add").blur();
      toast("已添加");
    } else if (act === "nodes") {
      nodesCache = nodesCache ? null : await invoke("control_nodes", { test: false });
    } else if (act === "test") {
      toast("测速中…");
      nodesCache = await invoke("control_nodes", { test: true });
      toast("测速完成");
    }
  } catch (err) {
    toast(String(err), true);
  }
  renderList();
}

async function applyRule(spec) {
  closeMenu();
  try {
    const n = await invoke("control_set", spec);
    const what = spec.target === "default" ? "已恢复按规则" : `已设为${TARGET_LABEL[spec.target]}`;
    toast(n ? `${what}，断开重连了 ${n} 条连接` : what);
  } catch (err) {
    toast(String(err), true);
  }
}

$("panel").addEventListener("contextmenu", (e) => {
  if (tab === "notes" && notesContext(e)) return;
});
$("panel").addEventListener("click", (e) => {
  if (tab === "notes" && !e.target.closest("[data-do]") && notesClick(e)) return;
  const inMenu = e.target.closest("#menu");
  const go = e.target.closest("#list [data-tab]");
  if (go && !e.target.closest("[data-do]")) {
    switchTab(go.dataset.tab, go.classList.contains("tile") ? go.getBoundingClientRect() : null);
    return;
  }
  const set = e.target.closest("[data-set]");
  if (set) {
    applyRule(JSON.parse(set.dataset.set));
    return;
  }
  const doEl = e.target.closest("[data-do]");
  if (doEl) {
    runAction(doEl);
    return;
  }
  const chip = e.target.closest("[data-chip]");
  if (chip) {
    e.stopPropagation();
    const app = snap.apps.find((a) => a.key === chip.dataset.chip);
    if (app) openMenu(chip, appMenu(app));
    return;
  }
  const act = e.target.closest("[data-act]");
  if (act) {
    e.stopPropagation();
    const ref = refs[Number(act.dataset.act)];
    if (ref) openMenu(act, connMenu(ref[0], ref[1]));
    return;
  }
  const node = e.target.closest("[data-node]");
  if (node) {
    invoke("control_select", { name: node.dataset.node })
      .then(() => { if (nodesCache) nodesCache.now = node.dataset.node; toast("已切换节点"); renderList(); })
      .catch((err) => toast(String(err), true));
    return;
  }
  const seg = e.target.closest("[data-seg]");
  const aiMore = e.target.closest("[data-ai-more]");
  if (aiMore) {
    const t = aiMore.dataset.aiMore;
    if (aiExpanded.has(t)) aiExpanded.delete(t);
    else aiExpanded.add(t);
    renderList(true);
    return;
  }
  if (seg?.dataset.seg === "sys-view") {
    sysView = seg.dataset.val;
    try { localStorage.setItem("mb-sys-view", sysView); } catch { /* ignore */ }
    if (sysView === "startup") { startupItems = null; if (!appInfo) refreshAppInfo(); }
    renderList(true);
    return;
  }
  if (seg?.dataset.seg === "sys-sort") {
    sysSort = seg.dataset.val;
    try { localStorage.setItem("mb-sys-sort", sysSort); } catch { /* ignore */ }
    seg.parentElement.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b === seg));
    updateSys();
    return;
  }
  if (seg?.dataset.seg === "tsort") {
    trafficSort = seg.dataset.val;
    try { localStorage.setItem("mb-traffic-sort", trafficSort); } catch { /* ignore */ }
    renderList(true);
    return;
  }
  if (seg) {
    const v = Number(seg.dataset.val);
    if (seg.dataset.seg === "srange") speedRange = v;
    if (seg.dataset.seg === "nrange") nodeRange = v;
    if (seg.dataset.seg === "tdays") { trafficDays = v; trafficDay = null; trafficData = null; loadTraffic(true); }
    renderList(true);
    return;
  }
  const tday = e.target.closest("[data-tday]");
  if (tday) {
    const d = tday.getAttribute("data-tday") || null;
    trafficDay = d && d !== trafficDay ? d : null; // clicking the picked day again goes back to the range
    loadTraffic(true);
    renderList(true);
    return;
  }
  const sheetRow = e.target.closest("[data-sheet-app]");
  if (sheetRow) {
    openAppSheet(sheetRow.dataset.sheetApp);
    return;
  }
  const sapp = e.target.closest("[data-sapp]");
  if (sapp) {
    speedApp = sapp.dataset.sapp || null;
    renderList(true);
    return;
  }
  const mv = e.target.closest("[data-mv]");
  if (mv) {
    moveItem(JSON.parse(mv.dataset.mv));
    return;
  }
  const guardEl = e.target.closest("[data-block],[data-unblock],[data-ack],[data-exclude],[data-imprm],[data-crm]");
  if (guardEl) {
    guardAction(guardEl);
    return;
  }
  const rm = e.target.closest("[data-rm]");
  if (rm) {
    invoke("control_remove", { id: rm.dataset.rm }).then(() => toast("已删除规则")).catch((err) => toast(String(err), true));
    return;
  }
  if (!inMenu) closeMenu();
}, true);

async function guardAction(el) {
  const d = el.dataset;
  try {
    if (d.block) {
      const { path, exe } = JSON.parse(d.block);
      const n = await invoke("guard_block", { path, exe });
      toast(`已禁止 ${exe} 直连${n ? `，断开了 ${n} 条连接` : ""}`);
    } else if (d.unblock) {
      await invoke("guard_unblock", { path: d.unblock });
      toast("已解除");
    } else if (d.ack) {
      await invoke("alert_ack", { key: d.ack });
    } else if (d.exclude) {
      await invoke("guard_add", { text: "!" + d.exclude });
      await invoke("alert_ack", { key: null });
      toast(`以后 ${d.exclude} 直连不再预警`);
    } else if (d.imprm) {
      await invoke("guard_import_set", { url: d.imprm, enabled: null, remove: true });
    } else if (d.crm) {
      const e = JSON.parse(d.crm);
      await invoke("guard_remove", { kind: e.kind, value: e.value });
    }
  } catch (err) {
    toast(String(err), true);
  }
  renderList();
}

// checkboxes in the guard tab
$("list").addEventListener("change", async (e) => {
  const el = e.target;
  try {
    if (el.dataset.opt) {
      const s = { ...(snap.guard?.settings || {}) };
      s[el.dataset.opt] = el.checked;
      if (el.dataset.opt === "gfw" && el.checked) toast("正在下载 gfw 列表…");
      await invoke("guard_options", { builtin: !!s.builtin, gfw: !!s.gfw, forceProxy: !!s.forceProxy, notify: !!s.notify });
    } else if (el.dataset.imp) {
      await invoke("guard_import_set", { url: JSON.parse(el.dataset.imp).url, enabled: el.checked, remove: false });
    }
  } catch (err) {
    toast(String(err), true);
  }
});

$("list").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  if (e.target.id === "g-add") runAction({ dataset: { do: "add" } });
  if (e.target.id === "g-import") runAction({ dataset: { do: "import" } });
});

$("list").addEventListener("click", (e) => {
  if (e.target.closest("[data-chip],[data-act],[data-set],[data-do],[data-node],[data-rm],.banner,[data-block],[data-unblock],[data-ack],[data-exclude],[data-imprm],[data-crm],.g-opt,.g-row,.alert-card")) return;
  const more = e.target.closest("[data-more]");
  if (more) {
    showAll.add(more.dataset.more);
    renderList();
    return;
  }
  const head = e.target.closest(".app-head");
  if (!head || tab !== "conns" || head.parentElement.classList.contains("gone")) return;
  const key = head.parentElement.dataset.key;
  if (openApps.has(key)) openApps.delete(key);
  else openApps.add(key);
  renderList();
});

// ---------- tabs (order and visibility come from settings) ----------

const TAB_DEFS = {
  conns: { label: "连接", count: true },
  listen: { label: "监听", count: true },
  rules: { label: "出口", count: true },
  guard: { label: "预警", count: true },
  speed: { label: "网速" },
  traffic: { label: "流量" },
  nodes: { label: "节点" },
};

// pages of the manager window (sidebar), with a small line icon each
const NAV = [
  ["home", "总览", "M3 12l9-8 9 8M5 10v10h14V10"],
  ["conns", "连接", "M4 6h16M4 12h16M4 18h10"],
  ["listen", "监听端口", "M12 3v18M3 12h18"],
  ["rules", "出口控制", "M5 12h14M13 6l6 6-6 6"],
  ["guard", "泄漏预警", "M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z"],
  ["speed", "网速", "M3 17l5-6 4 4 8-9"],
  ["traffic", "流量", "M6 20V10M12 20V4M18 20v-7"],
  ["nodes", "节点", "M12 20a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM12 8v4l3 2"],
  ["sys", "系统监控", "M6 20V12M10 20V6M14 20V10M18 20V4"],
  ["ai", "AI 额度", "M12 3l2.5 5.5L20 11l-5.5 2.5L12 19l-2.5-5.5L4 11l5.5-2.5z"],
  ["notes", "通知", "M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0"],
  ["settings", "设置", "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19 12h2M3 12h2M12 3v2M12 19v2M17 7l1.4-1.4M5.6 18.4L7 17M17 17l1.4 1.4M5.6 5.6L7 7"],
];

function visibleTabs() {
  if (MANAGER) return NAV.map((n) => n[0]);
  const list = (sget("panel.tabs", null) || Object.keys(TAB_DEFS).map((id) => ({ id, on: true })));
  const tabs = list.filter((t) => t.on && TAB_DEFS[t.id]).map((t) => t.id);
  return tabs.length ? tabs : ["conns"];
}

function renderTabs() {
  const tabs = visibleTabs();
  if (MANAGER) {
    if (!tabs.includes(tab)) tab = "home";
    document.body.dataset.page = tab;
    if (tab === "settings") {
      // the sidebar becomes the settings categories, with a way back
      $("tabs").innerHTML = settingsNavHtml();
      return;
    }
    const counts = { conns: true, guard: true, notes: true };
    $("tabs").innerHTML = NAV.map(([id, label, d]) => `<button class="tab nav ${id === tab ? "active" : ""}" data-tab="${id}">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>
      <span>${label}</span>${counts[id] ? `<span id="tab-${id}" class="mono nav-count">0</span>` : ""}</button>`).join("");
    document.body.dataset.page = tab;
    return;
  }
  if (tab !== "settings" && !PAGE_TABS.has(tab) && !tabs.includes(tab)) tab = tabs[0];
  if (!tabs.includes(netTab)) netTab = tabs[0];
  renderPages();
  $("tabs").innerHTML = tabs
    .map((id) => `<button class="tab ${id === tab ? "active" : ""}" data-tab="${id}">${TAB_DEFS[id].label}${TAB_DEFS[id].count ? ` <span id="tab-${id}" class="mono">0</span>` : ""}</button>`)
    .join("");
}

function switchTab(name, zoomFrom = null) {
  if (!MANAGER && name === "settings") {
    // the island's gear opens the settings in the desktop window
    invoke("open_manager", { page: "settings" }).catch(() => {});
    return;
  }
  const crossing = MANAGER && (tab === "settings") !== (name === "settings");
  if (MANAGER && name === "settings" && tab !== "settings") setBack = tab;
  tab = name;
  if (crossing) {
    renderTabs();
    playIn($("tabs"), name === "settings" ? "nav-fwd" : "nav-back");
  }
  if (!PAGE_TABS.has(name) && name !== "settings" && name !== "home") netTab = name;
  renderPages();
  document.querySelectorAll(".tab[data-tab]").forEach((x) => x.classList.toggle("active", x.dataset.tab === name));
  $("gear").classList.toggle("active", name === "settings");
  if (MANAGER) {
    document.body.dataset.page = name;
    if (name === "settings") {
      refreshMonitors();
      refreshFonts();
      refreshAppInfo();
      groupOptions = null;
    }
    if (name === "ai") invoke("ai_get").then((a) => { aiState = a || []; renderList(true); }).catch(() => {});
    try { history.replaceState(null, "", "#" + name); } catch { /* ignore */ }
  }
  $("list").scrollTop = 0;
  Sheet.close();
  // a card opened its page: the page grows out of the card; otherwise the items rise in one by one
  if (zoomFrom) Cascade.play(null);
  else Cascade.play($("list"));
  renderList(true);
  if (zoomFrom) sharedZoom(zoomFrom, $("list"));
  if (MANAGER) renderPanel();
  else measurePanel();
  ensureSysPoll();
  updateViz();
  if (tour.on && expanded) renderTour();
}

// a very narrow island: the wheel scrolls the tab row sideways
$("tabs").addEventListener("wheel", (e) => {
  const t = $("tabs");
  if (MANAGER || t.scrollWidth <= t.clientWidth || !e.deltaY) return;
  t.scrollLeft += e.deltaY;
  e.preventDefault();
}, { passive: false });
$("tabs").addEventListener("click", (e) => {
  const cat = e.target.closest("[data-setcat]");
  if (cat) return setCategory(cat.dataset.setcat);
  if (e.target.closest("[data-setback]")) return switchTab(setBack === "settings" ? "home" : setBack);
  const b = e.target.closest("[data-tab]");
  if (b) switchTab(b.dataset.tab);
});
$("gear").addEventListener("click", () => {
  if (!MANAGER) {
    invoke("open_manager", { page: "settings" }).catch(() => {});
    return;
  }
  switchTab("settings");
});

let glassWanted = null;
let zoomNow = 100;

// Glass on: hand the backend a frame channel; off: drop it (no screen capture at all).
function applyGlass() {
  if (MANAGER) return;
  const on = sget("appearance.glass", true) !== false;
  if (on === glassWanted) return;
  glassWanted = on;
  if (on) startGlass();
  else {
    invoke("glass_off").catch(() => {});
    island.classList.remove("glass");
    glassFrames = 0;
  }
}

let pageRestored = MANAGER;
function applySettings(s) {
  S = s;
  if (!pageRestored) {
    // open on the page used last time
    pageRestored = true;
    try {
      const p = localStorage.getItem("mb-page");
      if (PAGE_TABS.has(p)) tab = p;
    } catch { /* ignore */ }
  }
  document.documentElement.style.setProperty("--q-ok", sget("ai.ring.colors.ok", "#34d399"));
  document.documentElement.style.setProperty("--q-warn", sget("ai.ring.colors.warn", "#fbbf24"));
  document.documentElement.style.setProperty("--q-crit", sget("ai.ring.colors.crit", "#f87171"));
  applyTheme();
  applyGlass();
  applyMotion();
  maybeStartTour();
  const scale = sget("appearance.scale", 100);
  if (scale !== zoomNow && !MANAGER) {
    zoomNow = scale;
    invoke("set_zoom", { scale }).catch(() => {});
  }
  renderTabs();
  if (!editing) fitWindow();
  renderPill();
  if (tab === "settings") renderList(true);
  else renderPanel();
  applySize();
}

listen("settings", (e) => applySettings(e.payload));
Charts.attach($("list"));

// a newer version was found by the daily check
listen("update", (e) => {
  updateInfo = e.payload;
  fireFlash("update", `MicaBar 有新版本 ${updateInfo.version}，在设置里更新`, "info");
  if (tab === "settings") renderList(true);
});
invoke("settings_get").then(applySettings).catch(() => {});
// opened straight on the settings (manager.html#settings/...): load what switchTab would have
// (version and install type on the About page, monitors, fonts)
if (MANAGER && tab === "settings") {
  refreshMonitors();
  refreshFonts();
  refreshAppInfo();
}

$("search").addEventListener("input", (e) => {
  query = e.target.value.trim();
  renderList();
});

$("exit-row").addEventListener("click", (e) => {
  if (e.target.closest("#refresh")) {
    invoke("refresh_exit").catch(() => {});
    if (exitState) exitState.checking = true;
    renderExit();
    renderPill();
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    if (editing) {
      endEdit(false);
      return;
    }
    $("search").value = "";
    query = "";
    $("search").blur();
    if (!MANAGER) invoke("collapse_island").catch(() => {});
  }
});

listen("ai", (e) => {
  aiState = e.payload || [];
  renderPill();
  if (tab === "ai" || tab === "home") renderList(true);
});
listen("media", (e) => {
  media = e.payload || null;
  renderPill();
  if (tab === "home") renderList(true);
  else if (tab === "music") renderList();
  updateViz();
});
invoke("ai_get").then((a) => { aiState = a || []; }).catch(() => {});
invoke("media_get").then((m) => { media = m || null; }).catch(() => {});

listen("island", (e) => {
  if (MANAGER || editing) return;
  if (e.payload) {
    expand();
    // opened by the hotkey the window has keyboard focus: start on the active tab
    setTimeout(() => {
      if (document.hasFocus()) document.querySelector(curPage() === "net" ? ".tab.active" : ".pg.active")?.focus({ preventScroll: true });
    }, 60);
  } else {
    collapse();
  }
});
// ---------- first-run tour ----------
// Steps on the island itself: a card in place of the pill while collapsed, a card at the bottom of
// the open panel with a pulsing ring on what the step is about. Shown once (general.tourDone),
// again from 设置 → 关于. While it runs the backend keeps the panel open and lets the collapsed
// card take clicks (like a notification card).

function tourSteps() {
  const hk = esc(sget("interaction.hotkey", "Ctrl+Alt+N"));
  const pass = sget("interaction.passthrough", false);
  const click = sget("interaction.expand", "hover") === "click";
  const hold = Math.round(Number(sget("interaction.holdMs", 260)) || 260);
  const steps = [
    { id: "hello", title: "欢迎使用 MicaBar", body: "这座「岛」停在屏幕边上：收起时显示出口 IP、网速这些，展开后是完整的面板。花半分钟看看怎么用吧。" },
    { id: "open", title: "展开和收起", body: `${pass ? "现在是穿透模式：鼠标碰不到岛，只能用快捷键展开。" : click ? "平时点一下岛就会展开，点外面的地方就收起。" : "平时把鼠标在岛上停一会儿就会展开，移开就收起；收起时它不挡下面的点击。"}随时按 <kbd>${hk}</kbd> 也能展开 / 收起。<br>教程进行时，面板会一直开着。` },
  ];
  if (visiblePages().length > 1) steps.push({ id: "pages", target: "#pages", title: "切换页面", body: "顶部这排图标是各个页面：网络、系统、音乐、AI 额度、通知……点一下试试。页面的顺序和开关在设置里改。" });
  if (visiblePages().includes("net")) steps.push({ id: "tabs", target: "#tabs", net: true, title: "网络页的页签", body: "连接、网速、流量、节点……在网速页的图上左右移动鼠标，能看每一秒的数值；点下面的程序能看它的详情。" });
  steps.push({ id: "grab", target: "island", title: "拖着走", body: `按住面板不动约 ${hold} 毫秒，就能把整个岛抓起来：往左右甩换位置；往屏幕中间拉开，还能拖到另一条边上松手。收起时按住 <kbd>Alt</kbd> 再长按胶囊也行。现在就可以试试。` });
  steps.push({ id: "gear", target: "#gear", title: "设置与总览", body: "右上角的齿轮打开桌面窗口：总览、每天的流量、节点测速和全部设置都在那里。托盘图标右键也能打开。" });
  steps.push({ id: "legal", title: "最后：免责声明", legal: true, body: `MicaBar 由 <b>${AUTHOR}</b> 个人开发，以 MIT 许可开源（${REPO.replace("https://", "")}）。<br>软件按「现状」提供，不附带任何担保。它只在本机读取连接、进程和通知；「出口控制」「防泄漏」会改代理软件的覆写和防火墙规则，并可能重启代理软件。使用中造成的任何损失，作者不承担责任。完整的说明在 设置 → 关于。` });
  return steps;
}

function renderTour() {
  if (MANAGER) return;
  const el = $("tour");
  document.querySelectorAll(".tour-hl").forEach((x) => x.classList.remove("tour-hl"));
  island.classList.remove("tour-ring");
  if (!tour.on) {
    island.classList.remove("touring");
    el.innerHTML = "";
    return;
  }
  const steps = tourSteps();
  tour.step = Math.max(0, Math.min(tour.step, steps.length - 1));
  const N = steps.length - 1; // the welcome card is step 0, not counted
  const btn = (act, label, cls = "ghost") => `<button class="${cls}" data-tour="${act}">${label}</button>`;
  const card = (title, body, acts, head = "", legal = false) => `<div class="tour-card glow-card">
      ${head ? `<div class="tour-step">${head}</div>` : ""}<div class="tour-title">${title}</div>
      <div class="${legal ? "tour-legal" : "tour-body"} in">${body}</div><div class="tour-acts">${acts}</div></div>`;
  island.classList.add("touring");
  if (!expanded) {
    const s = steps[tour.step];
    el.innerHTML = tour.step === 0
      ? card(s.title, s.body, `${btn("skip", "跳过")}<span class="sp"></span>${btn("start", "开始 →", "primary")}`, "新手教程")
      : card("教程还没看完", `第 ${tour.step} / ${N} 步：${esc(s.title)}`, `${btn("skip", "结束教程")}<span class="sp"></span>${btn("resume", "继续", "primary")}`, "新手教程");
    el.style.setProperty("--tour-w", Math.min(window.innerWidth - 16, TOUR_W) + "px");
    tour.h = Math.ceil(el.offsetHeight);
    applySize();
    return;
  }
  if (tour.step === 0) tour.step = 1;
  const s = steps[tour.step];
  if (s.target === "island") island.classList.add("tour-ring");
  else if (s.target) document.querySelector(s.target)?.classList.add("tour-hl");
  const dots = `<span class="tour-dots">${steps.slice(1).map((_, k) => `<i class="${k + 1 === tour.step ? "on" : ""}"></i>`).join("")}</span><span>${tour.step} / ${N}</span>`;
  const last = tour.step === N;
  const acts = s.legal
    ? `${btn("repo", "打开仓库")}<span class="sp"></span>${btn("prev", "上一步")}${btn("done", "知道了", "primary")}`
    : `${btn("skip", "跳过")}<span class="sp"></span>${tour.step > 1 ? btn("prev", "上一步") : ""}${btn("next", last ? "完成" : "下一步", "primary")}`;
  el.innerHTML = card(s.title, s.body, acts, dots, s.legal);
}

function tourGo(d) {
  const steps = tourSteps();
  const i = Math.max(1, tour.step + d);
  if (i >= steps.length) return endTour();
  tour.step = i;
  if (steps[i].net && curPage() !== "net") switchPage("net"); // the tabs only exist on the network page
  renderTour();
}

function startTour() {
  if (MANAGER || editing) return;
  hideNotes();
  tour.on = true;
  tour.step = expanded ? 1 : 0;
  invoke("tour_mode", { on: true, open: false }).catch(() => {});
  invoke("log_js", { msg: "tour start" }).catch(() => {});
  renderTour();
  applySize();
}

function endTour() {
  tour.on = false;
  tour.step = 0;
  invoke("tour_mode", { on: false, open: false }).catch(() => {});
  invoke("settings_set", { patch: { general: { tourDone: true } } }).catch(() => {});
  renderTour();
  pillLast = "";
  renderPill();
  applySize();
}

// first start: once the settings are in, if the tour was never finished or skipped
function maybeStartTour() {
  if (MANAGER || tour.on || maybeStartTour.done || !S) return;
  maybeStartTour.done = true;
  if (!sget("general.tourDone", false)) setTimeout(() => { if (!tour.on && !sget("general.tourDone", false)) startTour(); }, 1800);
}

// a grab during the tour collapses the island: continue after it lands (past the grab step)
function tourAfterGrab() {
  if (!tour.on) return;
  if (tourSteps()[tour.step]?.id === "grab") tour.step++;
  renderTour();
  if (tour.step >= 1) setTimeout(() => invoke("tour_mode", { on: true, open: true }).catch(() => {}), 450);
}

if (!MANAGER) {
  $("tour").addEventListener("click", (e) => {
    const b = e.target.closest("[data-tour]");
    if (!b) return;
    const a = b.dataset.tour;
    if (a === "start" || a === "resume") {
      if (tour.step === 0) tour.step = 1;
      invoke("tour_mode", { on: true, open: true }).catch(() => {});
    } else if (a === "next") tourGo(1);
    else if (a === "prev") tourGo(-1);
    else if (a === "repo") invoke("open_url", { url: REPO }).catch(() => {});
    else if (a === "skip" || a === "done") endTour();
  });
  listen("tour-start", () => { tour.on = false; startTour(); });
}

// the panel never scrolls as a whole (only #list inside it does); focus() could shift it
$("panel").addEventListener("scroll", () => { if ($("panel").scrollTop) $("panel").scrollTop = 0; });

// click-to-open: the window receives the press itself while the cursor is over the pill
// ---------- grab and fling ----------
// Long press on the island (the open panel, the pill in click mode, or Alt + the collapsed pill)
// grabs it: it shrinks to the pill under the pointer and slides along the edge. Flung right it
// moves one spot right (centre -> right wall, left wall -> centre), flung left the other way;
// at a wall it bounces a few times, back to the centre it springs.
// Pulled well away from its edge it comes loose and follows the pointer anywhere; a bar shows the
// spot it will dock at (the nearest edge, left / middle / right third), and let go it flies there.

const EDGE_M = 10; // gap to the screen side at the start / end spot (CSS px)
const DETACH = 100; // pointer this far from the docked edge: the island comes loose (CSS px)
const ANCHORS = ["start", "center", "end"];
const fling = { active: false, phase: "", pos: 0, vel: 0, samples: [], from: "center", target: "", w: 0, bumps: 0, raf: 0, free: false, px: 0, py: 0, dock: "" };

// the island's monitor in screen CSS px (WB = bottom of the work area, above the taskbar)
function monRect() {
  const L = screen.left ?? screen.availLeft ?? 0, T = screen.top ?? screen.availTop ?? 0;
  const W = screen.width, H = screen.height;
  return { L, T, R: L + W, B: T + H, WB: (screen.availTop ?? T) + (screen.availHeight || H) };
}

// where a loose island let go at (px, py) docks: the nearest edge, and the third along it
function dockTarget(px, py) {
  const m = monRect();
  const d = { top: py - m.T, bottom: m.WB - py, left: px - m.L, right: m.R - px };
  const dock = Object.keys(d).reduce((a, b) => (d[b] < d[a] ? b : a));
  const vertical = dock === "left" || dock === "right";
  const f = vertical ? (py - m.T) / Math.max(1, m.WB - m.T) : (px - m.L) / Math.max(1, m.R - m.L);
  return { dock, anchor: f < 1 / 3 ? "start" : f > 2 / 3 ? "end" : "center" };
}

// the landing bar for a dock target, in screen CSS px
function hintRect(t) {
  const m = monRect();
  const len = 120, th = 5, off = 3;
  if (t.dock === "left" || t.dock === "right") {
    const y = t.anchor === "start" ? m.T + EDGE_M : t.anchor === "end" ? m.WB - len - EDGE_M : m.T + (m.WB - m.T - len) / 2;
    return { x: t.dock === "left" ? m.L + off : m.R - th - off, y, w: th, h: len };
  }
  const x = t.anchor === "start" ? m.L + EDGE_M : t.anchor === "end" ? m.R - len - EDGE_M : m.L + (m.R - m.L - len) / 2;
  return { x, y: t.dock === "top" ? m.T + off : m.WB - th - off, w: len, h: th };
}

let dockHint = null;
function showDockHint(t) {
  if (!dockHint) {
    dockHint = document.createElement("div");
    dockHint.id = "dock-hint";
    document.body.appendChild(dockHint);
  }
  const r = hintRect(t);
  dockHint.style.left = r.x - window.screenX + "px";
  dockHint.style.top = r.y - window.screenY + "px";
  dockHint.style.width = r.w + "px";
  dockHint.style.height = r.h + "px";
  dockHint.classList.add("on");
}
function hideDockHint() {
  dockHint?.classList.remove("on");
}
let press = null; // pending long press {sx, sy, timer, id}
let suppressClick = false;

// edge geometry in screen CSS px along the island's axis
function edgeAxis() {
  const vertical = shape().vertical;
  const origin = vertical ? (screen.top ?? screen.availTop ?? 0) : (screen.left ?? screen.availLeft ?? 0);
  const extent = vertical ? screen.height : screen.width;
  return { vertical, origin, extent };
}

function slotPos(anchor, w) {
  const { origin, extent } = edgeAxis();
  if (anchor === "start") return origin + EDGE_M;
  if (anchor === "end") return origin + extent - w - EDGE_M;
  return origin + (extent - w) / 2;
}

function startGrab(e) {
  press = null;
  if (fling.active || editing) return;
  const wasOpen = expanded;
  const { vertical } = edgeAxis();
  expanded = false;
  closeMenu();
  island.classList.remove("expanded", "peek", "touring");
  island.classList.add("collapsed", "grabbed", "flinging");
  pillLast = "";
  renderPill();
  const r = islandRect(false);
  fling.w = vertical ? r.h : r.w;
  fling.from = sget("shape.anchor", "center");
  const p = vertical ? e.screenY : e.screenX;
  fling.pos = p - fling.w / 2;
  fling.vel = 0;
  fling.samples = [{ p, t: performance.now() }];
  fling.phase = "drag";
  fling.active = true;
  fling.free = false;
  fling.dock = "";
  suppressClick = true;
  try { island.setPointerCapture(e.pointerId); } catch { /* ignore */ }
  invoke("grab_start").catch(() => {});
  invoke("log_js", { msg: `grab (${wasOpen ? "open panel" : "pill"}) from ${fling.from}` }).catch(() => {});
  applySize();
}

function cancelPress() {
  if (press) clearTimeout(press.timer);
  press = null;
}

// Is the pointer on a scrollbar of the pressed element or one of its scrolling parents?
// (Holding a scrollbar still must not grab the island.)
function onScrollbar(e) {
  for (let el = e.target; el && el !== document.body; el = el.parentElement) {
    if (!(el instanceof HTMLElement)) continue;
    const r = el.getBoundingClientRect();
    const barW = el.offsetWidth - el.clientWidth - el.clientLeft * 2;
    const barH = el.offsetHeight - el.clientHeight - el.clientTop * 2;
    if (barW > 0 && e.clientX >= r.left + el.clientLeft + el.clientWidth) return true;
    if (barH > 0 && e.clientY >= r.top + el.clientTop + el.clientHeight) return true;
  }
  return false;
}

island.addEventListener("pointerdown", (e) => {
  if (MANAGER || editing || fling.active || e.button !== 0) return;
  // controls, the scrub chart, the sheet and the tour card are not handles for grabbing the island
  if (e.target.closest("input, select, textarea, #menu, #tour, .sheet, .sheet-scrim, [data-nograb]")) return;
  if (onScrollbar(e)) return;
  const ev = { pointerId: e.pointerId, screenX: e.screenX, screenY: e.screenY };
  const hold = Math.min(1000, Math.max(150, Number(sget("interaction.holdMs", 260)) || 260));
  press = { sx: e.screenX, sy: e.screenY, target: e.target, timer: setTimeout(() => startGrab(ev), hold) };
});

// Images (flags, covers, app icons) must not start the browser's own drag and drop: a long press
// on a flag dragged the picture out (it could be dropped on the desktop as cn.svg) and cancelled
// the grab.
island.addEventListener("dragstart", (e) => e.preventDefault());

// anything scrolling while the button is down (scrollbar drag, wheel) is not a grab
document.addEventListener("scroll", () => { if (press) cancelPress(); }, true);

document.addEventListener("pointermove", (e) => {
  if (press && Math.hypot(e.screenX - press.sx, e.screenY - press.sy) > 6) cancelPress();
  if (!fling.active || fling.phase !== "drag") return;
  fling.px = e.screenX;
  fling.py = e.screenY;
  if (!fling.free) {
    // pulled far enough away from its edge: it comes loose
    const m = monRect(), dock = shape().dock;
    const away = { top: e.screenY - m.T, bottom: m.WB - e.screenY, left: e.screenX - m.L, right: m.R - e.screenX }[dock];
    if (away > DETACH) {
      fling.free = true;
      island.classList.add("free");
      invoke("grab_free").catch(() => {});
      invoke("log_js", { msg: "grabbed island came loose from its edge" }).catch(() => {});
    }
  }
  if (fling.free) {
    showDockHint(dockTarget(e.screenX, e.screenY));
    applySize();
    return;
  }
  const { vertical } = edgeAxis();
  const p = vertical ? e.screenY : e.screenX;
  const lo = slotPos("start", fling.w), hi = slotPos("end", fling.w);
  let pos = p - fling.w / 2;
  // a little give past the walls
  if (pos < lo) pos = lo - Math.sqrt(lo - pos) * 2;
  if (pos > hi) pos = hi + Math.sqrt(pos - hi) * 2;
  fling.pos = pos;
  const now = performance.now();
  fling.samples.push({ p, t: now });
  while (fling.samples.length > 2 && now - fling.samples[0].t > 90) fling.samples.shift();
  applySize();
});

document.addEventListener("pointerup", (e) => {
  if (press) {
    // a short press: on a live item it acts on it; otherwise, in click mode, it opens the island
    const t = press.target;
    cancelPress();
    if (!expanded && !fling.active && liveClick(t)) return;
    if (!expanded && !fling.active && sget("interaction.expand", "hover") === "click") invoke("pill_click").catch(() => {});
    return;
  }
  if (fling.active && fling.phase === "drag") release();
});

// swallow the click that ends a grab (it might land on a button)
document.addEventListener("click", (e) => {
  if (suppressClick) {
    suppressClick = false;
    e.stopPropagation();
    e.preventDefault();
  }
}, true);

// a loose island let go: it flies to the spot the bar showed, then docks on that edge
function releaseFree() {
  const t = dockTarget(fling.px, fling.py);
  const r = hintRect(t);
  // aim a little inside the bar, so the island arrives against the edge and settles from there
  const inward = { top: [0, 1], bottom: [0, -1], left: [1, 0], right: [-1, 0] }[t.dock];
  const tx = r.x + r.w / 2 + inward[0] * 22, ty = r.y + r.h / 2 + inward[1] * 22;
  const fx = fling.px, fy = fling.py;
  fling.target = t.anchor;
  fling.dock = t.dock;
  fling.phase = "anim";
  island.classList.remove("grabbed");
  const dur = document.documentElement.classList.contains("no-anim") ? 0 : 260;
  const t0 = performance.now();
  const step = (now) => {
    const k = dur ? Math.min(1, (now - t0) / dur) : 1;
    const e = 1 - Math.pow(1 - k, 3); // ease out
    fling.px = fx + (tx - fx) * e;
    fling.py = fy + (ty - fy) * e;
    applySize();
    if (k < 1) fling.raf = requestAnimationFrame(step);
    else endFling();
  };
  fling.raf = requestAnimationFrame(step);
}

function release() {
  if (fling.free) return releaseFree();
  const s = fling.samples;
  const a = s[0], b = s[s.length - 1];
  const v = b && a && b.t > a.t ? (b.p - a.p) / (b.t - a.t) : 0; // px per ms
  let i = ANCHORS.indexOf(fling.from);
  if (i < 0) i = 1;
  if (v > 0.45) i = Math.min(2, i + 1);
  else if (v < -0.45) i = Math.max(0, i - 1);
  else {
    // no real fling: the nearest spot to where it was let go
    let best = 1, d = Infinity;
    ANCHORS.forEach((an, k) => { const dd = Math.abs(slotPos(an, fling.w) - fling.pos); if (dd < d) { d = dd; best = k; } });
    i = best;
  }
  fling.target = ANCHORS[i];
  fling.vel = v;
  fling.bumps = 0;
  fling.phase = "anim";
  island.classList.remove("grabbed");
  let last = performance.now();
  const step = (now) => {
    const dt = Math.min(32, now - last);
    last = now;
    if (stepFling(dt)) {
      applySize();
      fling.raf = requestAnimationFrame(step);
    } else {
      endFling();
    }
  };
  fling.raf = requestAnimationFrame(step);
}

// one physics step; false when it has come to rest
function stepFling(dt) {
  const target = slotPos(fling.target, fling.w);
  if (fling.target === "center") {
    // spring to the middle, a little overshoot
    const k = 0.00042, c = 0.026;
    fling.vel += (-k * (fling.pos - target) - c * fling.vel) * dt;
    fling.pos += fling.vel * dt;
    return !(Math.abs(fling.pos - target) < 0.6 && Math.abs(fling.vel) < 0.03);
  }
  // to a wall: pulled towards it, bouncing off it ("dun dun dun"), each bounce smaller
  const dir = fling.target === "end" ? 1 : -1;
  if (fling.vel * dir < 0.9 && fling.bumps === 0) fling.vel = 0.9 * dir; // at least a decent throw
  fling.vel += 0.0065 * dir * dt;
  fling.pos += fling.vel * dt;
  if ((fling.pos - target) * dir >= 0) {
    fling.pos = target;
    if (Math.abs(fling.vel) < 0.22 || fling.bumps >= 4) return false;
    fling.vel = -fling.vel * 0.42;
    fling.bumps++;
    bump(dir);
  }
  return true;
}

function bump(dir) {
  const { vertical } = edgeAxis();
  island.style.transformOrigin = vertical ? (dir > 0 ? "50% 100%" : "50% 0%") : (dir > 0 ? "100% 50%" : "0% 50%");
  island.classList.remove("bump-x", "bump-y");
  void island.offsetWidth; // restart the animation
  island.classList.add(vertical ? "bump-y" : "bump-x");
}

function endFling() {
  cancelAnimationFrame(fling.raf);
  const target = fling.target;
  const dock = fling.free ? fling.dock : "";
  fling.active = false;
  fling.free = false;
  fling.phase = "";
  hideDockHint();
  island.classList.remove("grabbed", "free");
  if (dock) {
    // a new edge: the window moves there; the island keeps "flinging" (no position transition,
    // so it doesn't slide across from the old window's coordinates) while it reshapes for the
    // edge, then settles with a small bounce
    clearTimeout(endFling.t);
    endFling.t = setTimeout(() => island.classList.remove("flinging"), 380);
    island.classList.remove("land");
    void island.offsetWidth;
    island.classList.add("land");
  } else {
    island.classList.remove("flinging");
  }
  // remember the spot; the backend refits the window there
  if (S?.shape) {
    S.shape.anchor = target;
    if (dock) S.shape.dock = dock;
  }
  invoke("grab_end", { anchor: target, dock: dock || null }).catch(() => {});
  pillLast = "";
  renderPill();
  fitted = "";
  fitWindow();
  applySize();
  tourAfterGrab();
}

// ---------- layout editing: drag the edges of the panel / pill ----------
// The backend hands the whole window the mouse while editing; the window is temporarily as big
// as the largest panel, so the handles can be dragged up to the limits.

function editRect() {
  return islandRect(editPart === "panel");
}

function showEditPart() {
  if (editPart === "panel") {
    expanded = true;
    island.classList.add("expanded");
    island.classList.remove("collapsed");
    panelHNow = panelH();
    renderPanel();
  } else {
    expanded = false;
    closeMenu();
    island.classList.remove("expanded");
    island.classList.add("collapsed");
    pillLast = "";
    renderPill();
  }
  applySize();
}

function startEdit() {
  if (editing || MANAGER) return;
  editing = true;
  editPart = "panel";
  const cur = {};
  for (const k of Object.keys(SIZE_DEFAULTS)) cur[k] = sz(k);
  editSize = cur;
  document.body.classList.add("editing");
  $("edit").hidden = false;
  fitWindow();
  setTimeout(showEditPart, 80);
}

async function endEdit(save) {
  if (!editing) return;
  const sizes = editSize;
  editing = false;
  editSize = null;
  document.body.classList.remove("editing");
  $("edit").hidden = true;
  if (save) await setPath("size", sizes);
  await invoke("layout_edit", { on: false }).catch(() => {});
  expanded = false;
  island.classList.remove("expanded");
  island.classList.add("collapsed");
  fitted = "";
  fitWindow();
  pillLast = "";
  renderPill();
  applySize();
  if (save) toast("大小已保存");
}

// which edges can be dragged depends on the docked edge (the island stays centred on it)
function editEdges(dock) {
  return { top: ["e", "w", "s"], bottom: ["e", "w", "n"], left: ["e", "n", "s"], right: ["w", "n", "s"] }[dock] || ["e", "w", "s"];
}

function renderEditUi() {
  const box = $("edit");
  if (!editing || !box) return;
  const r = editRect();
  const kW = editPart === "panel" ? "panelW" : "pillW";
  const kH = editPart === "panel" ? "panelH" : "pillH";
  const handles = editEdges(r.dock).map((e) => {
    const hz = e === "e" || e === "w";
    const x = e === "e" ? r.x + r.w - 5 : e === "w" ? r.x - 5 : r.x + r.w / 2 - 22;
    const y = e === "s" ? r.y + r.h - 5 : e === "n" ? r.y - 5 : r.y + r.h / 2 - 22;
    return `<div class="eh ${hz ? "ew" : "ns"}" data-edge="${e}" style="left:${x}px;top:${y}px"></div>`;
  }).join("");
  const w = Math.round(r.w), h = Math.round(r.h);
  const ty = editPart === "panel" ? r.y + r.h / 2 - 40 : r.dock === "bottom" ? r.y - 110 : r.y + r.h + 24;
  box.innerHTML = `${handles}<div class="e-frame" style="left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px"></div>
    <div class="e-bar" style="top:${Math.max(4, ty)}px">
      <div class="e-row">${segmented("epart", editPart, [["panel", "展开面板"], ["pill", "收起的胶囊"]])}<span class="mono e-size">${w} × ${h}</span></div>
      <div class="e-row"><span class="e-hint">拖动${editPart === "panel" ? "面板" : "胶囊"}的边缘调整大小${editPart === "pill" && !(editSize[kW] > 0) ? "（宽度现在是自动的）" : ""}</span></div>
      <div class="e-row"><button class="ghost" data-edo="reset">恢复默认</button><button class="ghost" data-edo="cancel">取消</button><button class="primary" data-edo="done">完成</button></div>
    </div>`;
  box.dataset.kw = kW;
  box.dataset.kh = kH;
}

let drag = null;
$("edit").addEventListener("pointerdown", (e) => {
  const hd = e.target.closest("[data-edge]");
  if (!hd) return;
  const r = editRect();
  const edge = hd.dataset.edge;
  const kW = editPart === "panel" ? "panelW" : "pillW";
  const kH = editPart === "panel" ? "panelH" : "pillH";
  drag = { edge, x: e.clientX, y: e.clientY, w: r.w, h: r.h, kW, kH, dock: r.dock };
  hd.setPointerCapture(e.pointerId);
  island.classList.add("dragging");
  e.preventDefault();
});
$("edit").addEventListener("pointermove", (e) => {
  if (!drag) return;
  const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
  const centredX = drag.dock === "top" || drag.dock === "bottom";
  const centredY = !centredX;
  let w = drag.w, h = drag.h;
  if (drag.edge === "e") w = drag.w + (centredX ? 2 * dx : dx);
  if (drag.edge === "w") w = drag.w - (centredX ? 2 * dx : dx);
  if (drag.edge === "s") h = drag.h + (centredY ? 2 * dy : dy);
  if (drag.edge === "n") h = drag.h - (centredY ? 2 * dy : dy);
  const clamp = (k, v) => Math.round(Math.min(SIZE_LIMITS[k][1], Math.max(k === "pillW" ? 120 : SIZE_LIMITS[k][0], v)));
  if (drag.edge === "e" || drag.edge === "w") editSize[drag.kW] = clamp(drag.kW, w);
  else editSize[drag.kH] = clamp(drag.kH, h);
  if (editPart === "panel") panelHNow = panelH();
  applySize();
});
const stopDrag = () => {
  if (!drag) return;
  drag = null;
  island.classList.remove("dragging");
  if (editPart === "panel") renderPanel();
};
$("edit").addEventListener("pointerup", stopDrag);
$("edit").addEventListener("pointercancel", stopDrag);
$("edit").addEventListener("click", (e) => {
  const seg = e.target.closest("[data-seg='epart']");
  if (seg) {
    editPart = seg.dataset.val;
    showEditPart();
    return;
  }
  const b = e.target.closest("[data-edo]");
  if (!b) return;
  if (b.dataset.edo === "done") endEdit(true);
  else if (b.dataset.edo === "cancel") endEdit(false);
  else if (b.dataset.edo === "reset") {
    editSize = { ...SIZE_DEFAULTS };
    showEditPart();
  }
});
listen("layout-edit", (e) => (e.payload ? startEdit() : endEdit(false)));

listen("maximized", (e) => setMaximized(e.payload));

// animations: follow Windows "animation effects" (auto), or always on / off (设置 → 面板主题 → 动效)
function applyMotion() {
  const m = sget("appearance.motion", "auto");
  const off = m === "off" || (m === "auto" && snap?.animations === false);
  document.documentElement.classList.toggle("no-anim", off);
}

// ---------- keyboard ----------
// Tab / Shift+Tab: native focus order. ←/→ on the tab bar switch pages. ↑/↓ move between rows
// and controls of the list. Enter / Space on a row opens it. Esc closes the island.
function focusables() {
  return [...$("list").querySelectorAll('.app-head, button:not(:disabled), input, select, [tabindex="0"]')].filter((el) => el.offsetParent);
}

document.addEventListener("keydown", (e) => {
  if (!expanded) return;
  const a = document.activeElement;
  const typing = a && (a.tagName === "INPUT" || a.tagName === "SELECT");
  if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && a?.classList.contains("pg")) {
    const pages = visiblePages();
    const i = pages.indexOf(a.dataset.page) + (e.key === "ArrowRight" ? 1 : -1);
    const id = pages[(i + pages.length) % pages.length];
    switchPage(id);
    document.querySelector(`.pg[data-page="${id}"]`)?.focus({ preventScroll: true });
    e.preventDefault();
  } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && a?.classList.contains("tab")) {
    const tabs = [...document.querySelectorAll(".tab")];
    const i = tabs.indexOf(a) + (e.key === "ArrowRight" ? 1 : -1);
    const t = tabs[(i + tabs.length) % tabs.length];
    switchTab(t.dataset.tab);
    document.querySelector(`.tab[data-tab="${t.dataset.tab}"]`)?.focus({ preventScroll: true });
    e.preventDefault();
  } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && !typing) {
    const list = focusables();
    if (!list.length) return;
    const i = list.indexOf(a);
    const next = e.key === "ArrowDown" ? (i < 0 ? 0 : Math.min(list.length - 1, i + 1)) : (i <= 0 ? -1 : i - 1);
    if (next < 0) document.querySelector(".tab.active")?.focus({ preventScroll: true });
    else {
      list[next].focus();
      list[next].scrollIntoView({ block: "nearest" });
    }
    e.preventDefault();
  } else if ((e.key === "Enter" || e.key === " ") && (a?.classList.contains("app-head") || a?.classList.contains("sp-row"))) {
    a.click();
    e.preventDefault();
  }
});
listen("game", (e) => {
  gameOn = !!e.payload;
  island.classList.toggle("game", gameOn);
  if (nst.on) renderNote(); // privacy: hide / show the text
});
// cursor resting on the collapsed (click-through) pill: let the window below show through
listen("peek", (e) => island.classList.toggle("peek", !!e.payload && !expanded));

let snapErr = 0;
listen("snapshot", (e) => {
  try { onSnapshot(e); } catch (err) {
    if (snapErr++ < 3) invoke("log_js", { msg: `${MANAGER ? "manager" : "island"} snapshot error: ${err && err.stack || err}` }).catch(() => {});
  }
});
function onSnapshot(e) {
  snap = e.payload;
  exitState = snap.exit;
  if (!!snap.maximized !== maximized) setMaximized(snap.maximized);
  recordSpeed(snap);
  applyMotion();
  checkFlashes(snap);
  renderPill();
  renderPanel();
}

listen("exit", (e) => {
  exitState = e.payload;
  renderPill();
  renderPanel();
});

invoke("get_snapshot").then((s) => {
  if (s) {
    snap = s;
    exitState = s.exit;
  }
  renderPill();
}).catch(() => renderPill());

// ---------- glass backdrop ----------

const glassCanvas = $("glass");
const gctx = glassCanvas.getContext("2d", { alpha: false });
let glassFrames = 0;

// Darker tint over bright backgrounds keeps text readable; lighter over dark ones keeps it glassy.
function adaptTint(px, w, h) {
  const vw = window.innerWidth || 1;
  const r = islandRect(expanded);
  const sx = w / vw, sy = h / (window.innerHeight || 1);
  const x0 = Math.max(0, Math.floor(r.x * sx)), x1 = Math.min(w, Math.ceil((r.x + r.w) * sx));
  const y0 = Math.max(0, Math.floor(r.y * sy)), y1 = Math.min(h, Math.ceil((r.y + r.h) * sy));
  let sum = 0, n = 0;
  for (let y = y0; y < y1; y += 3) {
    for (let x = x0; x < x1; x += 3) {
      const i = (y * w + x) * 4;
      sum += 0.2126 * px[i] + 0.7152 * px[i + 1] + 0.0722 * px[i + 2];
      n++;
    }
  }
  const luma = n ? sum / n / 255 : 0.3;
  // dark glass needs more tint over bright backgrounds, white glass over dark ones
  let a = document.documentElement.dataset.theme === "light" ? 0.5 + (1 - luma) * 0.32 : 0.4 + luma * 0.42;
  // opacity setting: 50 = as computed, 100 = opaque, 20 = very clear
  a = glassOpacity >= 50 ? a + (1 - a) * ((glassOpacity - 50) / 50) : a * (glassOpacity / 50);
  island.style.setProperty("--tint-a", a.toFixed(3));
}

// Per-pixel brightness limit on the blurred backdrop, so text stays readable where the panel sits
// over a contrasting area (white glass over a black terminal, black glass over a white page) even
// when the rest of the backdrop averages out fine: white glass lifts dark areas to a floor, black
// glass dims bright ones to a ceiling; hues stay.
const LEGIBLE = { light: { normal: 150, strong: 185 }, dark: { normal: 100, strong: 70 } };
function legible(px) {
  const mode = sget("appearance.legible", "normal");
  if (mode === "off") return;
  const light = document.documentElement.dataset.theme === "light";
  const lim = (light ? LEGIBLE.light : LEGIBLE.dark)[mode === "strong" ? "strong" : "normal"];
  for (let i = 0; i < px.length; i += 4) {
    const r = px[i], g = px[i + 1], b = px[i + 2];
    const L = (r * 54 + g * 183 + b * 19) >> 8;
    if (light) {
      if (L < lim) {
        const t = (lim - L) / (255 - L);
        px[i] = r + (255 - r) * t;
        px[i + 1] = g + (255 - g) * t;
        px[i + 2] = b + (255 - b) * t;
      }
    } else if (L > lim) {
      const f = lim / L;
      px[i] = r * f;
      px[i + 1] = g * f;
      px[i + 2] = b * f;
    }
  }
}

function startGlass() {
  const ch = new T.core.Channel();
  ch.onmessage = (msg) => {
    const ab = msg instanceof ArrayBuffer ? msg : msg?.buffer instanceof ArrayBuffer ? msg.buffer : new Uint8Array(msg).buffer;
    const dv = new DataView(ab);
    const w = dv.getUint32(0, true), h = dv.getUint32(4, true);
    if (!w || !h || ab.byteLength < 8 + w * h * 4) return;
    const px = new Uint8ClampedArray(ab, 8, w * h * 4);
    if (glassCanvas.width !== w || glassCanvas.height !== h) {
      glassCanvas.width = w;
      glassCanvas.height = h;
    }
    legible(px);
    gctx.putImageData(new ImageData(px, w, h), 0, 0);
    adaptTint(px, w, h);
    if (glassFrames++ === 0) {
      island.classList.add("glass");
      invoke("log_js", { msg: `glass first frame ${w}x${h}` }).catch(() => {});
    }
  };
  invoke("start_glass", { onFrame: ch }).catch((e) => invoke("log_js", { msg: `start_glass failed: ${e}` }));
}

function logView(tag) {
  invoke("log_js", { msg: `${tag} inner=${window.innerWidth}x${window.innerHeight} dpr=${window.devicePixelRatio} island=${island.offsetWidth}x${island.offsetHeight}` }).catch(() => {});
}

let resizeTimer = null;
window.addEventListener("resize", () => {
  if (!MANAGER && !editing) applySize();
  reportRect();
  if (MANAGER) {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => renderList(true), 150); // charts follow the window width
  } else logView("resize");
});
if (MANAGER) {
  listen("goto", (e) => {
    const [page, cat] = String(e.payload || "home").split("/");
    if (cat && SET_CATS.includes(cat)) {
      setCat = cat;
      try { localStorage.setItem("mb-set-cat", cat); } catch { /* ignore */ }
      if (tab === "settings") { renderTabs(); renderList(true); return; }
    }
    switchTab(page);
  });
  $("side").addEventListener("click", (e) => {
    const d = e.target.closest("[data-do]");
    if (d) runAction(d);
  });
  // the home page has live numbers: refresh it with every snapshot (other pages already do)
}
fitWindow();
applySize();
if (S) applyGlass(); // otherwise applySettings starts it once settings arrive
logView("start");
