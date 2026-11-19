"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;

const EXPANDED_W = 600;
const EXPANDED_H = 540;
const MAX_CONNS_PER_APP = 60;

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

let regionNames = null;
try { regionNames = new Intl.DisplayNames(["zh-CN"], { type: "region" }); } catch { /* ignore */ }

// ---------- settings ----------

let S = null; // settings document from the backend
const sget = (path, dflt) => path.split(".").reduce((o, k) => (o == null ? o : o[k]), S) ?? dflt;
const itemOn = (listPath, id) => (sget(listPath, []) || []).some((i) => i.id === id && i.on);

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
  const p = app.path;
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
  // measure natural pill width
  const w = pill.scrollWidth;
  return Math.max(120, Math.min(window.innerWidth - 16, w));
}

// Window size in CSS px; the backend gets it in physical px (× devicePixelRatio, which also
// carries the Windows "text size" accessibility factor).
const WIN_W = EXPANDED_W + 40;
const WIN_H = EXPANDED_H + 30;

function fitWindow() {
  const dpr = window.devicePixelRatio || 1;
  invoke("fit_window", { w: Math.round(WIN_W * dpr), h: Math.round(WIN_H * dpr) }).catch(() => {});
  matchMedia(`(resolution: ${dpr}dppx)`).addEventListener("change", fitWindow, { once: true });
}

let panelH = EXPANDED_H;

// Expanded height follows the content (list rows), clamped to EXPANDED_H.
function measurePanel() {
  const list = $("list");
  const foot = $("foot");
  const inner = $("list-inner");
  const footH = foot.childElementCount ? foot.offsetHeight + 10 : 0;
  const need = list.offsetTop + (inner ? inner.offsetHeight : 120) + footH + 14;
  const h = Math.max(240, Math.min(EXPANDED_H, Math.ceil(need)));
  if (Math.abs(h - panelH) > 3) {
    panelH = h;
    if (expanded) applySize();
  }
}

let maximized = false; // a maximized window is in front (setting: shrink to a line)

// Effective collapsed shape and docked edge.
function shape() {
  const dock = sget("shape.dock", "top");
  let style = sget("shape.style", "capsule");
  if (maximized && sget("shape.lineWhenMaximized", false)) style = "line";
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
    w = EXPANDED_W;
    h = panelH;
  } else if (style === "line") {
    [w, h] = vertical ? [6, 140] : [140, 6];
  } else if (vertical) {
    w = 40;
    h = Math.max(60, Math.min(H - 16, pill.scrollHeight));
  } else {
    w = pillWidth();
    h = 36;
  }
  // capsule floats 6px off the edge; notch and line sit on it; an expanded panel floats unless notch
  const gap = open ? (style === "notch" ? 0 : 6) : style === "capsule" ? 6 : 0;
  let x = (W - w) / 2, y = (H - h) / 2;
  if (dock === "top") y = gap;
  else if (dock === "bottom") y = H - h - gap;
  else if (dock === "left") x = gap;
  else x = W - w - gap;
  return { x, y, w, h, style, dock, vertical };
}

// Both rectangles go to the backend, which runs the hover state machine (dwell, buttons, leave).
function reportRect() {
  const dpr = window.devicePixelRatio || 1;
  const phys = (r, pad) => [(r.x - pad) * dpr, (r.y - pad) * dpr, (r.w + 2 * pad) * dpr, (r.h + 2 * pad) * dpr];
  const c = islandRect(false);
  // the thin line gets a bigger hover zone than it looks
  invoke("set_hit_rects", { pill: phys(c, c.style === "line" ? 10 : 4), panel: phys(islandRect(true), 4) }).catch(() => {});
}

function applySize() {
  const r = islandRect(expanded);
  island.classList.toggle("notch", r.style === "notch");
  island.classList.toggle("line", r.style === "line" && !expanded);
  island.classList.toggle("vertical", r.vertical && !expanded);
  for (const d of ["top", "bottom", "left", "right"]) island.classList.toggle("dock-" + d, r.dock === d);
  island.style.setProperty("--ix", r.x + "px");
  island.style.setProperty("--iy", r.y + "px");
  island.style.width = r.w + "px";
  island.style.height = r.h + "px";
  if (!expanded && r.style === "capsule") island.style.borderRadius = r.h / 2 + "px";
  else island.style.borderRadius = "";
  reportRect();
}

function expand() {
  if (expanded) return;
  expanded = true;
  island.classList.add("expanded");
  island.classList.remove("collapsed");
  $("panel").scrollTop = 0;
  renderPanel();
  applySize();
}

function collapse() {
  if (!expanded) return;
  expanded = false;
  closeMenu();
  confirmDisable = false;
  island.classList.remove("expanded");
  island.classList.add("collapsed");
  $("search").blur();
  applySize();
}

// ---------- pill ----------

function shortRate(b) {
  if (!b || b < 1) return "0";
  if (b < 1024) return `${b.toFixed(0)}B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(b < 10240 ? 1 : 0)}K`;
  return `${(b / 1024 / 1024).toFixed(1)}M`;
}

function clockText() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}${sget("pill.clockSeconds", false) ? ":" + p(d.getSeconds()) : ""}`;
}

// One pill item; "" when it has nothing to show.
function pillItem(id, target) {
  switch (id) {
    case "exit":
      return `${target?.cc ? flagImg(target.cc) : ""}<span class="mono ip-text">${esc(target?.ip || (target?.error ? "出口检测失败" : "检测中…"))}</span>`;
    case "conns":
      return `<span class="mono">${snap ? snap.connCount : 0}</span><span class="pill-unit">连接</span>`;
    case "speed":
      return `<span class="mono pill-speed"><span class="u">↑</span>${shortRate(snap?.up)} <span class="d">↓</span>${shortRate(snap?.down)}</span>`;
    case "latency": {
      const L = snap?.latency;
      if (!L?.at) return "";
      if (L.ms == null) return `<span class="lat bad">节点${esc(L.error || "无响应")}</span>`;
      const c = L.ms < 200 ? "good" : L.ms < 500 ? "mid" : "bad";
      return `<span class="lat ${c} mono">${L.ms}ms</span>`;
    }
    case "leaks": {
      const n = snap?.guard?.unacked || 0;
      return n ? `<span class="pill-alert">⚠ 泄漏 ${n}</span>` : "";
    }
    case "cpu":
      return snap?.sys?.cpu != null ? `<span class="pill-unit">CPU</span><span class="mono">${Math.round(snap.sys.cpu)}%</span>` : "";
    case "mem":
      return snap?.sys?.memTotal ? `<span class="pill-unit">内存</span><span class="mono">${Math.round((snap.sys.memUsed / snap.sys.memTotal) * 100)}%</span>` : "";
    case "clock":
      return `<span class="clock clock-${esc(sget("pill.clockStyle", "simple"))}">${clockText()}</span>`;
  }
  return "";
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

function renderPill() {
  const p = snap?.proxy;
  const ex = exitState;
  const target = p?.detected ? ex?.proxy : ex?.direct;
  let dot = "dot";
  if (ex?.checking) dot += " busy";
  if (target?.ip) dot += p?.detected ? " ok" : " warn";
  else if (target?.error) dot += " err";
  if (snap?.guard?.unacked) dot = "dot err busy";
  // the thin-line style shows the same status as a colour
  const st = snap?.guard?.unacked || (!target?.ip && target?.error) ? "err" : target?.ip ? (p?.detected ? "ok" : "warn") : "idle";
  for (const s of ["ok", "warn", "err", "idle"]) island.classList.toggle("st-" + s, s === st);

  let html;
  if (flash.msg && Date.now() < flash.until) {
    html = `<span class="flash-msg ${flash.msg.level}">${decorate(flash.msg.text)}</span>`;
  } else {
    flash.msg = null;
    const items = (sget("pill.items", null) || [{ id: "exit", on: true }, { id: "conns", on: true }, { id: "leaks", on: true }])
      .filter((i) => i.on)
      .map((i) => pillItem(i.id, target))
      .filter(Boolean);
    html = items.map((x) => `<span class="pill-item">${x}</span>`).join(`<span class="pill-sep"></span>`);
  }
  pill.innerHTML = `<span class="${dot}"></span>${html}`;
  if (!expanded) applySize();
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

function renderExit() {
  const p = snap?.proxy;
  const ex = exitState || {};
  let html = "";
  if (p?.detected) {
    html += exitCard("代理出口", ex.proxy, { checking: ex.checking, sameAs: ex.direct?.ip });
    html += exitCard("直连出口", ex.direct, { checking: ex.checking, direct: true });
  } else {
    html += exitCard("出口 IP（未检测到代理）", ex.direct, { checking: ex.checking, direct: true });
  }
  const spin = ex.checking ? "spin" : "";
  const when = ex.lastCheck ? ago(ex.lastCheck) : "";
  html = `<div style="display:flex;gap:8px;flex:1;position:relative">${html}
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
    const title = t === "proxy" && node ? ` title="NetIsland 节点：${esc(node)}"` : "";
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
  if (app.isProxy || st === "unavailable") return "";
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
    <div class="m-actions end"><button class="${confirmDisable ? "danger" : "ghost"}" data-do="disable">${confirmDisable ? "再点一次：移除覆写并重启 Clash Party" : "停用出口控制"}</button></div>`;
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

function speedTabHtml() {
  const n = Math.min(speedRange, speedHist.t.length);
  if (n < 2) return `<div class="empty">正在收集网速数据…</div>`;
  const src = speedApp ? appHist.get(speedApp) : null;
  if (speedApp && !src) speedApp = null;
  const xs = speedHist.t.slice(-n);
  const up = (src ? src.up : speedHist.up).slice(-n);
  const down = (src ? src.down : speedHist.down).slice(-n);
  const chart = Charts.line({
    id: "speed", width: 566, height: 170, xs,
    series: [{ name: "下载", cls: "s1", values: down }, { name: "上传", cls: "s2", values: up }],
    fmtY: (v) => fmtRate(v), fmtTick: tickRate, fmtX: hms, endLabels: true, binary: true,
  });
  const apps = [...appHist.entries()]
    .map(([key, h]) => ({ key, ...h, cur: (h.up[h.up.length - 1] || 0) + (h.down[h.down.length - 1] || 0), sum: h.up.slice(-n).reduce((a, b) => a + b, 0) + h.down.slice(-n).reduce((a, b) => a + b, 0) }))
    .filter((a) => a.sum > 0)
    .sort((a, b) => b.cur - a.cur || b.sum - a.sum)
    .slice(0, 15);
  const rows = apps.map((a) => {
    const series = a.up.slice(-n).map((u, i) => u + a.down.slice(-n)[i]);
    return `<div class="sp-row ${a.key === speedApp ? "sel" : ""}" data-sapp="${esc(a.key)}" tabindex="0">
      ${iconHtml(a)}<span class="sp-name">${esc(a.name)}</span>${Charts.spark(series)}
      <span class="speed mono">${speedHtml(a.up[a.up.length - 1], a.down[a.down.length - 1])}</span></div>`;
  }).join("");
  return `<div class="viz">
    <div class="viz-head">${segmented("srange", speedRange, [[60, "1 分钟"], [120, "2 分钟"], [300, "5 分钟"]])}
      <span class="viz-sel">${src ? `只看 <b>${esc(src.name)}</b> <button data-sapp="">✕ 看全部</button>` : "全部程序"}</span>
      <span class="legend"><span class="lg"><i class="key s1"></i>下载</span><span class="lg"><i class="key s2"></i>上传</span></span></div>
    ${chart}
    <div class="g-head" style="margin-top:8px">按程序（点一下只看它）</div>
    ${rows || `<div class="empty">这段时间没有程序在传数据</div>`}
  </div>`;
}

// ---------- traffic tab ----------

let trafficDays = 1;
let trafficData = null;
let trafficAt = 0;

function loadTraffic(force) {
  if (!force && Date.now() - trafficAt < 10_000) return;
  trafficAt = Date.now();
  invoke("traffic_query", { days: trafficDays })
    .then((r) => { trafficData = r; if (tab === "traffic") renderList(true); })
    .catch(() => {});
}

function trafficTabHtml() {
  loadTraffic(false);
  const r = trafficData;
  if (!r) return `<div class="empty">正在读取流量记录…</div>`;
  const P = (x) => x.pu + x.pd, D = (x) => x.du + x.dd;
  const tp = r.days.reduce((s, d) => s + P(d), 0), td = r.days.reduce((s, d) => s + D(d), 0);
  const tot = tp + td;
  const pct = (v) => (tot ? Math.round((v / tot) * 100) : 0);
  const head = `<div class="viz-head">${segmented("tdays", trafficDays, [[1, "今天"], [7, "最近 7 天"]])}
    <span class="legend"><span class="lg"><i class="key s1"></i>代理 ${fmtBytes(tp)}（${pct(tp)}%）</span><span class="lg"><i class="key s2"></i>直连 ${fmtBytes(td)}（${pct(td)}%）</span></span></div>`;
  const summary = `<div class="t-total"><span class="t-label">合计</span><span class="t-value">${fmtBytes(tot)}</span>
    ${Charts.segbar([{ value: tp, cls: "s1" }, { value: td, cls: "s2" }], `代理 ${fmtBytes(tp)} · 直连 ${fmtBytes(td)}`)}</div>`;
  const chart = trafficDays > 1
    ? Charts.columns({
        id: "traffic", width: 566, height: 160, cats: r.days.map((d) => d.date),
        stacks: [{ name: "代理", cls: "s1", values: r.days.map(P) }, { name: "直连", cls: "s2", values: r.days.map(D) }],
        fmtY: fmtBytes, fmtTick: tickRate, binary: true, fmtCat: (d) => (d === r.days[r.days.length - 1].date ? "今天" : d.slice(5)),
      })
    : "";
  const rows = r.apps.filter((a) => P(a) + D(a) > 0).slice(0, 30).map((a) => `<div class="t-row">
      ${iconHtml({ path: a.path, name: a.name })}<span class="sp-name">${esc(a.name)}</span>
      ${Charts.segbar([{ value: P(a), cls: "s1" }, { value: D(a), cls: "s2" }], `代理 ${fmtBytes(P(a))} · 直连 ${fmtBytes(D(a))}`)}
      <span class="t-num mono">${fmtBytes(P(a) + D(a))}</span>
      <span class="t-sub mono">↑${fmtBytes(a.pu + a.du)} ↓${fmtBytes(a.pd + a.dd)}</span></div>`).join("");
  return `<div class="viz">${head}${summary}${chart}
    <div class="g-head" style="margin-top:8px">按程序${trafficDays > 1 ? "（7 天合计）" : "（今天）"}</div>
    ${rows || `<div class="empty">还没有流量记录</div>`}
    <div class="m-hint">「代理」= 经过代理节点的字节；其余算「直连」。数据每分钟存一次，存在本机。</div></div>`;
}

// ---------- node quality tab ----------

let latHist = [];
let latAt = -1;
let nodeRange = 120; // minutes

function loadLatency() {
  const at = snap?.latency?.at || 0;
  if (at === latAt) return;
  latAt = at;
  invoke("latency_history").then((h) => { latHist = h || []; if (tab === "nodes") renderList(true); }).catch(() => {});
}

function nodesTabHtml() {
  loadLatency();
  const L = snap?.latency;
  const now = Date.now();
  const hour = latHist.filter((s) => s.at > now - 3600_000);
  const ok = hour.filter((s) => s.ms != null).map((s) => s.ms).sort((a, b) => a - b);
  const avg = ok.length ? Math.round(ok.reduce((a, b) => a + b, 0) / ok.length) : null;
  const p95 = ok.length ? ok[Math.min(ok.length - 1, Math.floor(ok.length * 0.95))] : null;
  const loss = hour.length ? Math.round(((hour.length - ok.length) / hour.length) * 100) : null;
  const tile = (label, value, note = "") => `<div class="tile"><div class="tile-label">${label}</div><div class="tile-value">${value}</div>${note ? `<div class="tile-note">${note}</div>` : ""}</div>`;
  const tiles = `<div class="tiles">
    ${tile("当前延迟", L?.at ? (L.ms != null ? `${L.ms} ms` : `<span class="crit">✕ ${esc(L.error || "超时")}</span>`) : "—", L?.node ? decorate(L.node) : "")}
    ${tile("1 小时平均", avg != null ? `${avg} ms` : "—")}
    ${tile("1 小时 P95", p95 != null ? `${p95} ms` : "—")}
    ${tile("1 小时丢包", loss != null ? `${loss}%` : "—", hour.length ? `${hour.length} 次测速` : "")}
  </div>`;
  const pts = latHist.filter((s) => nodeRange >= 100000 || s.at > now - nodeRange * 60_000);
  const chart = pts.length >= 2
    ? Charts.line({
        id: "latency", width: 566, height: 170, xs: pts,
        series: [{ name: "延迟", cls: "s1", values: pts.map((s) => s.ms) }],
        markers: pts.map((s, i) => (s.ms == null ? { i, label: `${noFlags(s.node)} 无响应` } : null)).filter(Boolean),
        ref: { value: sget("flash.latencyMs", 800), label: `提示阈值 ${sget("flash.latencyMs", 800)} ms` },
        fmtY: (v) => `${Math.round(v)} ms`, fmtTick: (v) => `${v}`, fmtX: (s) => `${hm(s.at)} · ${noFlags(s.node)}`,
      })
    : `<div class="empty">${snap?.proxy?.mihomoOk ? "测速数据还不够，稍等一会儿" : "需要 mihomo 控制接口和代理出口节点才能测速"}</div>`;
  return `<div class="viz">
    <div class="viz-head">${segmented("nrange", nodeRange, [[30, "30 分钟"], [120, "2 小时"], [100000, "全部"]])}
      <span class="viz-sel">代理出口节点的延迟（${{ saver: 60, balanced: 30, smooth: 15 }[sget("general.profile", "balanced")]} 秒测一次）</span>
      <button class="ghost" data-do="lat-probe">立即测速</button></div>
    ${tiles}${chart}
    <div class="m-hint">✕ 表示那次测速没有响应。节点无响应 / 延迟超过阈值时会在胶囊上提示（设置里可改）。</div></div>`;
}

// ---------- settings page ----------

const PILL_ITEM_LABEL = {
  exit: "出口 IP + 国旗", conns: "连接数", speed: "实时网速", latency: "节点延迟",
  leaks: "泄漏数（有泄漏时才显示）", cpu: "CPU 占用", mem: "内存占用", clock: "时钟",
};

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

function sToggle(path, label, note = "") {
  return `<label class="s-row"><span class="s-label">${label}${note ? `<span class="s-note">${note}</span>` : ""}</span>
    <input type="checkbox" data-sp="${path}" ${sget(path, false) ? "checked" : ""}></label>`;
}

function sSelect(path, label, options, note = "") {
  const cur = String(sget(path, ""));
  return `<label class="s-row"><span class="s-label">${label}${note ? `<span class="s-note">${note}</span>` : ""}</span>
    <select data-sp="${path}">${options.map(([v, t]) => `<option value="${esc(v)}" ${String(v) === cur ? "selected" : ""}>${esc(t)}</option>`).join("")}</select></label>`;
}

function sNumber(path, label, min, max, step, unit, note = "") {
  return `<label class="s-row"><span class="s-label">${label}${note ? `<span class="s-note">${note}</span>` : ""}</span>
    <span class="s-num"><input type="number" data-sp="${path}" data-num="1" min="${min}" max="${max}" step="${step}" value="${esc(sget(path, min))}">${unit}</span></label>`;
}

function sColor(path, label) {
  return `<label class="s-row"><span class="s-label">${label}</span><input type="color" data-sp="${path}" value="${esc(sget(path, "#000000"))}"></label>`;
}

let fontsCache = [];
function refreshFonts() {
  if (fontsCache.length) return;
  invoke("fonts_list").then((f) => { fontsCache = f || []; if (tab === "settings") renderList(true); }).catch(() => {});
}

function sOrderList(path, labels) {
  const list = (sget(path, []) || []).filter((i) => labels[i.id]);
  return `<div class="s-order">${list.map((i, k) => `<div class="s-ord-row">
      <label><input type="checkbox" data-ol="${path}" data-id="${i.id}" ${i.on ? "checked" : ""}> ${labels[i.id]}</label>
      <span class="s-ord-btns"><button data-mv='${esc(JSON.stringify({ path, id: i.id, dir: -1 }))}' ${k === 0 ? "disabled" : ""}>↑</button><button data-mv='${esc(JSON.stringify({ path, id: i.id, dir: 1 }))}' ${k === list.length - 1 ? "disabled" : ""}>↓</button></span>
    </div>`).join("")}</div>`;
}

function settingsHtml() {
  if (!S) return `<div class="empty">正在读取设置…</div>`;
  const tabLabels = Object.fromEntries(Object.entries(TAB_DEFS).map(([k, v]) => [k, v.label]));
  return `
  <div class="s-sec"><div class="s-title">外观</div>
    ${sSelect("appearance.theme", "主题", [["auto", "跟随系统（白 / 黑磨砂）"], ["glass-dark", "黑色磨砂"], ["glass-light", "白色磨砂"], ...Object.entries(PRESETS).map(([k, p]) => [k, p.name]), ["custom", "自定义"]])}
    ${sget("appearance.theme") === "custom" ? `
      ${sSelect("appearance.custom.base", "　基调", [["dark", "深色"], ["light", "浅色"]])}
      ${sColor("appearance.custom.bg", "　底色")}
      ${sColor("appearance.custom.text", "　文字颜色")}
      ${sColor("appearance.custom.accent", "　强调色")}` : ""}
    ${sToggle("appearance.glass", "实时磨砂", "关掉后用纯色背景，同时停止截取屏幕（更省资源）")}
    <label class="s-row"><span class="s-label">背景不透明度<span class="s-note">50 = 默认；往右更不透明，往左更通透</span></span>
      <span class="s-num"><input type="range" data-sp="appearance.opacity" data-num="1" min="20" max="100" step="5" value="${esc(sget("appearance.opacity", 50))}"><span class="mono">${esc(sget("appearance.opacity", 50))}</span></span></label>
    <label class="s-row"><span class="s-label">字体<span class="s-note">留空 = 系统默认</span></span>
      <span class="s-num"><input class="s-text" list="font-list" data-sp="appearance.font" value="${esc(sget("appearance.font", ""))}" placeholder="例如 微软雅黑" spellcheck="false"></span></label>
    <datalist id="font-list">${fontsCache.map((f) => `<option value="${esc(f)}">`).join("")}</datalist>
    ${sSelect("appearance.scale", "界面缩放", [[90, "90%"], [100, "100%"], [110, "110%"], [125, "125%"], [150, "150%"]])}
  </div>
  <div class="s-sec"><div class="s-title">胶囊里显示什么<span class="s-note">勾选显示，箭头调顺序</span></div>
    ${sOrderList("pill.items", PILL_ITEM_LABEL)}
    ${sSelect("pill.clockStyle", "时钟样式", [["simple", "简洁"], ["digital", "数码管"], ["serif", "衬线"]])}
    ${sToggle("pill.clockSeconds", "时钟显示秒")}
  </div>
  <div class="s-sec"><div class="s-title">短暂提示<span class="s-note">发生时在胶囊上显示 5 秒，同类 1 分钟内只提示一次</span></div>
    ${sToggle("flash.exitChange", "出口 IP 变了")}
    ${sToggle("flash.nodeDown", "节点无响应")}
    ${sToggle("flash.latencyHigh", "节点延迟变高")}
    ${sNumber("flash.latencyMs", "　延迟超过", 100, 5000, 50, "ms")}
    ${sToggle("flash.speedSpike", "网速突增")}
    ${sNumber("flash.speedMBps", "　突增到超过", 1, 1000, 1, "MB/s")}
  </div>
  <div class="s-sec"><div class="s-title">面板页签<span class="s-note">勾选显示，箭头调顺序</span></div>
    ${sOrderList("panel.tabs", tabLabels)}
  </div>
  <div class="s-sec"><div class="s-title">外形</div>
    ${sSelect("shape.style", "收起时的样子", [["capsule", "胶囊（悬浮）"], ["notch", "刘海（贴边）"], ["line", "细线（只显示状态色）"]])}
    ${sSelect("shape.dock", "停靠位置", [["top", "屏幕上边"], ["bottom", "屏幕下边（任务栏上方）"], ["left", "屏幕左边"], ["right", "屏幕右边"]], "左右两边时，胶囊会变成竖着的刘海")}
    ${sToggle("shape.lineWhenMaximized", "前台窗口最大化时缩成细线")}
  </div>
  <div class="s-sec"><div class="s-title">交互</div>
    ${sSelect("interaction.expand", "怎么展开", [["hover", "鼠标停留"], ["click", "点击"]])}
    ${sNumber("interaction.hoverDelay", "　鼠标停留多久展开", 0, 2000, 50, "ms")}
    ${sToggle("interaction.passthrough", "穿透模式", "鼠标完全不触发，点击穿透到下面的窗口；只能用快捷键打开")}
    <div class="s-row"><span class="s-label">快捷键（展开 / 收起）${snap && !snap.hotkeyOk ? `<span class="s-note bad">没注册成功：可能被别的程序占用了</span>` : ""}</span>
      <button class="hk ${hotkeyRec ? "rec" : ""}" data-do="hotkey-rec">${hotkeyRec ? "请按下新的组合键…（Esc 取消）" : esc(sget("interaction.hotkey", "Ctrl+Alt+N"))}</button></div>
    ${sSelect("interaction.fullscreen", "全屏程序（游戏、视频）在前台时", [["pill", "显示胶囊，不自动展开"], ["hide", "完全隐藏"]])}
  </div>
  <div class="s-sec"><div class="s-title">常规</div>
    ${sToggle("general.autostart", "开机自启", "登录 10 秒后以管理员权限启动（计划任务，不弹 UAC）")}
    ${sSelect("general.monitor", "显示在哪块屏幕", [["primary", "主屏幕"], ["cursor", "跟随鼠标所在的屏幕"], ...monitorsCache.map((m) => [m.name, `${m.name.replace("\\\\.\\", "")}（${m.w}×${m.h}${m.primary ? "，主屏" : ""}）`])])}
    ${sSelect("general.profile", "资源档位", [["saver", "省电"], ["balanced", "均衡"], ["smooth", "流畅"]], "影响节点测速等后台任务的频率")}
  </div>
  <div class="s-sec"><div class="s-title">关于</div>
    <div class="s-row"><span class="s-label">NetIsland ${esc(appInfo?.version || "")}<span class="s-note">${appInfo ? (appInfo.installed ? "安装版" : "便携版（不能自动更新）") : ""}</span></span>
      <button class="ghost" data-do="update-check">检查更新</button></div>
    ${updateInfo?.available ? `<div class="s-row"><span class="s-label">有新版本 ${esc(updateInfo.version)}<span class="s-note">${esc((updateInfo.notes || "").slice(0, 120))}</span></span>
      ${updateInfo.installed ? `<button class="primary" data-do="update-install">下载并更新</button>` : `<button class="ghost" data-do="open-releases">去 GitHub 下载</button>`}</div>` : ""}
    ${sToggle("general.autoUpdate", "自动检查更新", "每天检查一次 GitHub 上的新版本，有新版本时在胶囊上提示")}
    <div class="s-row"><span class="s-label">日志与数据<span class="s-note">${esc(appInfo?.logDir || "")}</span></span>
      <span><button class="ghost" data-do="open-logs">打开目录</button> <button class="ghost" data-do="open-home">GitHub 主页</button></span></div>
  </div>
  <div class="m-actions end"><button class="ghost" data-do="settings-reset">恢复默认设置</button></div>`;
}

let appInfo = null;
let updateInfo = null;
function refreshAppInfo() {
  invoke("app_info").then((i) => { appInfo = i; if (tab === "settings") renderList(true); }).catch(() => {});
}
const REPO = "https://github.com/aileboeth/net-island";

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

// ---------- leak guard tab ----------

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
    ? "规则库里的网站要被判成直连时，改走 NetIsland 分组（不影响订阅已分到代理组的）"
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
  if (!listHover) {
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
$("list").addEventListener("mouseleave", () => {
  listHover = false;
  renderList();
});

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
  if (c.rule && /netisland-guard/.test(c.rule)) l2.push(`<span class="ni" title="规则库里的网站本该被判成直连，守卫让它改走代理">守卫：强制走代理</span>`);
  else if (c.rule && /netisland-/.test(c.rule)) l2.push(`<span class="ni">NetIsland 规则</span>`);
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
  const list = $("list");
  if (tab === "settings") {
    // the settings page only re-renders when settings change (not on every snapshot), so open
    // dropdowns and number fields are not destroyed while the user works with them
    if (!force) return;
    const scroll = list.scrollTop;
    list.innerHTML = `<div id="list-inner">${settingsHtml()}</div>`;
    list.scrollTop = scroll;
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
  if (tab === "speed" || tab === "traffic" || tab === "nodes") {
    const scroll = list.scrollTop;
    list.innerHTML = `<div id="list-inner">${tab === "speed" ? speedTabHtml() : tab === "traffic" ? trafficTabHtml() : nodesTabHtml()}</div>`;
    list.scrollTop = scroll;
    Charts.restore(list);
    measurePanel();
    return;
  }
  if (tab === "rules") {
    rows.push(rulesTabHtml());
  } else if (tab === "guard") {
    rows.push(guardTabHtml());
  } else if (tab === "conns") {
    const leaks = snap.guard?.unacked || 0;
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
  renderExit();
  renderProxyLine();
  const setCount = (id, v) => { const el = $("tab-" + id); if (el) el.textContent = v; };
  setCount("conns", snap?.connCount ?? 0);
  setCount("listen", snap?.listenCount ?? 0);
  setCount("rules", ctrl().rules.length);
  setCount("guard", snap?.guard?.unacked || 0);
  $("tab-guard")?.parentElement.classList.toggle("hot", !!snap?.guard?.unacked);
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
    if (act === "enable") {
      toast("正在写入覆写并重启 Clash Party…");
      closeMenu();
      await invoke("control_enable");
      toast("出口控制已启用");
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
    } else if (act === "update-check") {
      toast("正在检查更新…");
      updateInfo = await invoke("update_check");
      toast(updateInfo.available ? `有新版本 ${updateInfo.version}` : "已经是最新版本");
      renderList(true);
      return;
    } else if (act === "update-install") {
      toast("正在下载更新，完成后会自动重启…");
      await invoke("update_install");
      return;
    } else if (act === "open-releases") {
      invoke("open_url", { url: REPO + "/releases/latest" });
      return;
    } else if (act === "open-home") {
      invoke("open_url", { url: REPO });
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

$("panel").addEventListener("click", (e) => {
  const inMenu = e.target.closest("#menu");
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
  if (seg) {
    const v = Number(seg.dataset.val);
    if (seg.dataset.seg === "srange") speedRange = v;
    if (seg.dataset.seg === "nrange") nodeRange = v;
    if (seg.dataset.seg === "tdays") { trafficDays = v; trafficData = null; loadTraffic(true); }
    renderList(true);
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

function visibleTabs() {
  const list = (sget("panel.tabs", null) || Object.keys(TAB_DEFS).map((id) => ({ id, on: true })));
  const tabs = list.filter((t) => t.on && TAB_DEFS[t.id]).map((t) => t.id);
  return tabs.length ? tabs : ["conns"];
}

function renderTabs() {
  const tabs = visibleTabs();
  if (tab !== "settings" && !tabs.includes(tab)) tab = tabs[0];
  $("tabs").innerHTML = tabs
    .map((id) => `<button class="tab ${id === tab ? "active" : ""}" data-tab="${id}">${TAB_DEFS[id].label}${TAB_DEFS[id].count ? ` <span id="tab-${id}" class="mono">0</span>` : ""}</button>`)
    .join("");
}

function switchTab(name) {
  tab = name;
  document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("active", x.dataset.tab === name));
  $("gear").classList.toggle("active", name === "settings");
  $("list").scrollTop = 0;
  renderList(true);
}

$("tabs").addEventListener("click", (e) => {
  const b = e.target.closest("[data-tab]");
  if (b) switchTab(b.dataset.tab);
});
$("gear").addEventListener("click", () => {
  if (tab !== "settings") {
    refreshMonitors();
    refreshFonts();
    refreshAppInfo();
  }
  switchTab(tab === "settings" ? visibleTabs()[0] : "settings");
});

let glassWanted = null;
let zoomNow = 100;

// Glass on: hand the backend a frame channel; off: drop it (no screen capture at all).
function applyGlass() {
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

function applySettings(s) {
  S = s;
  applyTheme();
  applyGlass();
  const scale = sget("appearance.scale", 100);
  if (scale !== zoomNow) {
    zoomNow = scale;
    invoke("set_zoom", { scale }).catch(() => {});
  }
  renderTabs();
  renderPill();
  if (tab === "settings") renderList(true);
  else renderPanel();
}

listen("settings", (e) => applySettings(e.payload));
Charts.attach($("list"));

// a newer version was found by the daily check
listen("update", (e) => {
  updateInfo = e.payload;
  fireFlash("update", `NetIsland 有新版本 ${updateInfo.version}，在设置里更新`, "info");
  if (tab === "settings") renderList(true);
});
invoke("settings_get").then(applySettings).catch(() => {});

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
    $("search").value = "";
    query = "";
    $("search").blur();
    invoke("collapse_island").catch(() => {});
  }
});

listen("island", (e) => {
  if (e.payload) {
    expand();
    // opened by the hotkey the window has keyboard focus: start on the active tab
    setTimeout(() => { if (document.hasFocus()) document.querySelector(".tab.active")?.focus({ preventScroll: true }); }, 60);
  } else {
    collapse();
  }
});
// the panel never scrolls as a whole (only #list inside it does); focus() could shift it
$("panel").addEventListener("scroll", () => { if ($("panel").scrollTop) $("panel").scrollTop = 0; });

// click-to-open: the window receives the press itself while the cursor is over the pill
island.addEventListener("mousedown", () => {
  if (!expanded) invoke("pill_click").catch(() => {});
});

listen("maximized", (e) => {
  maximized = !!e.payload;
  applySize();
});

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
  if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && a?.classList.contains("tab")) {
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
  } else if ((e.key === "Enter" || e.key === " ") && a?.classList.contains("app-head")) {
    a.click();
    e.preventDefault();
  }
});
listen("game", (e) => island.classList.toggle("game", !!e.payload));

listen("snapshot", (e) => {
  snap = e.payload;
  exitState = snap.exit;
  if (snap.maximized !== maximized) {
    maximized = !!snap.maximized;
  }
  recordSpeed(snap);
  // follow Windows "animation effects"
  document.documentElement.classList.toggle("no-anim", snap.animations === false);
  checkFlashes(snap);
  renderPill();
  renderPanel();
});

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

window.addEventListener("resize", () => { reportRect(); logView("resize"); });
fitWindow();
applySize();
if (S) applyGlass(); // otherwise applySettings starts it once settings arrive
logView("start");
