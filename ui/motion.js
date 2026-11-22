"use strict";
// Motion helpers shared by the island and the desktop window: rolling counters, cards that tilt
// towards the pointer, the press / release spring on buttons, the bottom sheet, the card -> page
// zoom and the staggered cascade of list items. Everything is off when <html> has .no-anim.
// The pages are rebuilt from HTML strings every second, so each helper keeps its state outside
// the DOM and puts it back onto the new elements after a re-render.

const motionOff = () => document.documentElement.classList.contains("no-anim");

// ---------- rolling counter (odometer) ----------
// Odo.slot(key) renders the text last shown for that key; Odo.set(key, text) then rolls each digit
// from there to the new one (a column 0–9 sliding inside a one-digit window).
const Odo = (() => {
  const shown = new Map(); // key -> text on screen
  const isD = (c) => c >= "0" && c <= "9";
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const COL = "<i>0</i><i>1</i><i>2</i><i>3</i><i>4</i><i>5</i><i>6</i><i>7</i><i>8</i><i>9</i>";
  // digits sit in reels; everything else (".", units, "—") is plain text
  const build = (text, start) => [...text].map((c, i) => (isD(c)
    ? `<span class="odo-d"><span class="odo-s" style="--d:${start ? start[i] : c}">${COL}</span></span>`
    : `<span class="odo-c">${c === " " ? "&nbsp;" : esc(c)}</span>`)).join("");
  const shape = (t) => [...t].map((c) => (isD(c) ? "#" : c)).join("");

  // the unit ("KB/s") sits after the reels as plain text
  function slot(key, cls = "") {
    const s = shown.get(key) || { t: "", u: "" };
    return `<span class="odo ${cls}" data-odo="${esc(key)}" aria-label="${esc(s.t + (s.u ? " " + s.u : ""))}"><span class="odo-n" data-t="${esc(s.t)}">${build(s.t)}</span><span class="odo-u">${esc(s.u)}</span></span>`;
  }

  function apply(el, text) {
    const cur = el.dataset.t ?? "";
    if (cur === text) return;
    el.dataset.t = text;
    if (shape(cur) === shape(text) && cur) {
      const reels = el.querySelectorAll(".odo-s");
      let k = 0;
      for (const c of text) if (isD(c)) reels[k++]?.style.setProperty("--d", c);
      return;
    }
    // the layout changed (more digits, another unit): start the digits from the old ones where
    // they line up from the right, else from 0, and roll them in the next frame
    const old = [...cur].reverse();
    const start = [...text].reverse().map((c, i) => (isD(c) ? (isD(old[i]) ? old[i] : "0") : c)).reverse();
    el.innerHTML = build(text, motionOff() ? null : start);
    if (motionOff()) return;
    void el.offsetWidth;
    let k = 0;
    const reels = el.querySelectorAll(".odo-s");
    for (const c of text) if (isD(c)) reels[k++]?.style.setProperty("--d", c);
  }

  function set(key, text, unit = "", root = document) {
    text = String(text ?? "");
    shown.set(key, { t: text, u: unit });
    const el = root.querySelector(`.odo[data-odo="${CSS.escape(key)}"]`);
    if (!el) return;
    el.setAttribute("aria-label", text + (unit ? " " + unit : ""));
    const u = el.querySelector(".odo-u");
    if (u && u.textContent !== unit) u.textContent = unit;
    const n = el.querySelector(".odo-n");
    if (n) apply(n, text);
  }
  return { slot, set };
})();

// ---------- cards that tilt towards the pointer, with a light spot under it ----------
const Tilt = (() => {
  let cur = null; // { el, key, rx, ry, mx, my }
  const all = () => [...document.querySelectorAll(".tilt")];

  function put(el, s, now) {
    if (now) el.classList.add("tilt-now");
    el.style.setProperty("--rx", s.rx.toFixed(2) + "deg");
    el.style.setProperty("--ry", s.ry.toFixed(2) + "deg");
    el.style.setProperty("--mx", s.mx.toFixed(1) + "%");
    el.style.setProperty("--my", s.my.toFixed(1) + "%");
    el.classList.add("tilting");
    if (now) { void el.offsetWidth; el.classList.remove("tilt-now"); }
  }

  function leave() {
    if (!cur) return;
    const el = cur.el;
    cur = null;
    if (!el?.isConnected) return;
    el.classList.remove("tilting");
    el.style.setProperty("--rx", "0deg");
    el.style.setProperty("--ry", "0deg");
  }

  document.addEventListener("pointermove", (e) => {
    const el = e.target instanceof Element ? e.target.closest(".tilt") : null;
    if (cur && cur.el !== el) leave();
    if (!el || motionOff()) return;
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return;
    const px = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const py = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
    // bigger cards tilt less, so the far edge never moves more than a few pixels
    const max = Math.min(Number(el.dataset.tilt) || 7, 2400 / Math.max(r.width, r.height));
    const s = { el, key: all().indexOf(el), rx: (0.5 - py) * 2 * max, ry: (px - 0.5) * 2 * max, mx: px * 100, my: py * 100 };
    cur = s;
    put(el, s, false);
  }, { passive: true });
  document.addEventListener("pointerleave", leave);
  window.addEventListener("blur", leave);

  // after a re-render: the card under the pointer is a new element, give it the same tilt at once
  function restore() {
    if (!cur || cur.el.isConnected) return;
    const el = all()[cur.key];
    if (!el) { cur = null; return; }
    cur.el = el;
    put(el, cur, true);
  }
  return { restore, leave };
})();

// ---------- press: squeeze to 0.96 with an inner shadow, spring back past 1 on release ----------
const PRESS_SEL = "button:not(.no-press), .tile.go, .sp-row, .top-row, [data-press]";
(() => {
  let pressed = null;
  document.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !(e.target instanceof Element)) return;
    pressed = e.target.closest(PRESS_SEL);
    if (pressed) { pressed.classList.remove("pop"); pressed.classList.add("pressing"); }
  }, true);
  const up = () => {
    const el = pressed;
    pressed = null;
    if (!el) return;
    el.classList.remove("pressing");
    if (motionOff() || !el.isConnected) return;
    el.classList.remove("pop");
    void el.offsetWidth;
    el.classList.add("pop");
    clearTimeout(el._popT);
    el._popT = setTimeout(() => el.classList.remove("pop"), 520);
  };
  document.addEventListener("pointerup", up, true);
  document.addEventListener("pointercancel", up, true);
})();

// ---------- spring (per millisecond) ----------
// k = stiffness, c = damping; c / (2·√k) ≈ 0.9 is the "heavy fluid" feel: no wobble, soft stop.
function springTo(get, setv, target, { v = 0, k = 0.0005, c = 0.04, done } = {}) {
  let x = get(), vel = v, last = performance.now(), raf = 0, stopped = false;
  if (motionOff()) { setv(target); done?.(); return { stop() {} }; }
  const step = (now) => {
    if (stopped) return;
    const dt = Math.min(32, now - last);
    last = now;
    for (let t = 0; t < dt; t += 4) {
      const h = Math.min(4, dt - t);
      vel += (-k * (x - target) - c * vel) * h;
      x += vel * h;
    }
    if (Math.abs(x - target) < 0.4 && Math.abs(vel) < 0.02) {
      setv(target);
      done?.();
      return;
    }
    setv(x);
    raf = requestAnimationFrame(step);
  };
  raf = requestAnimationFrame(step);
  return { stop() { stopped = true; cancelAnimationFrame(raf); } };
}

// ---------- bottom sheet ----------
// Rises from the bottom of a host (the island panel / the desktop window's page). Drag the top bar:
// past the top it stretches like a rubber band; let go and it settles on the anchor the throw is
// heading for (closed / half / full), faster throws skip to the next one.
const Sheet = (() => {
  let host = null, el = null, scrim = null, body = null;
  let vis = 0, anim = null, onClose = null, drag = null;
  const RUBBER = 70;

  const full = () => Math.max(160, (host?.clientHeight || 400) - 28);
  const half = () => Math.min(full(), Math.max(230, Math.round((host?.clientHeight || 400) * 0.6)));
  const anchors = () => [0, half(), full()];

  // the sheet's height is what shows (its body scrolls inside), so nothing hides below the host
  function paint() {
    if (!el) return;
    el.style.height = Math.max(0, vis).toFixed(1) + "px";
    scrim.style.opacity = Math.max(0, Math.min(1, vis / half())).toFixed(3);
    el.classList.toggle("at-full", vis > full() - 4);
  }

  function to(target, v = 0) {
    anim?.stop();
    anim = springTo(() => vis, (x) => { vis = x; paint(); }, target, {
      v, k: 0.00055, c: 0.042,
      done: () => { anim = null; if (target === 0) teardown(); },
    });
  }

  function teardown() {
    const cb = onClose;
    onClose = null;
    el?.remove(); scrim?.remove();
    el = scrim = body = null;
    host = null;
    vis = 0;
    cb?.();
  }

  function open(h, html, { onClose: oc, onClick } = {}) {
    if (el && host !== h) { anim?.stop(); teardown(); }
    onClose = oc || null;
    if (!el) {
      host = h;
      scrim = document.createElement("div");
      scrim.className = "sheet-scrim";
      scrim.addEventListener("click", () => close());
      el = document.createElement("div");
      el.className = "sheet";
      el.setAttribute("role", "dialog");
      el.innerHTML = `<div class="sheet-bar" data-nograb title="上下拖动；往下甩关闭"><i></i></div><div class="sheet-body"></div>`;
      body = el.querySelector(".sheet-body");
      host.append(scrim, el);
      el.addEventListener("click", (e) => onClick?.(e));
      el.querySelector(".sheet-bar").addEventListener("pointerdown", startDrag);
      el.addEventListener("pointerdown", (e) => { if (e.target.closest("[data-sheet-drag]") && !e.target.closest("button")) startDrag(e); });
      vis = 0;
      paint();
    }
    body.innerHTML = html;
    to(half());
  }

  // a click is a press and a release on the same element: no rebuild while a button is held
  let held = false;
  document.addEventListener("pointerdown", (e) => { held = !!el && el.contains(e.target); }, true);
  document.addEventListener("pointerup", () => { held = false; }, true);
  document.addEventListener("pointercancel", () => { held = false; }, true);

  function update(html) {
    if (!body || drag || held) return;
    const s = body.scrollTop;
    body.innerHTML = html;
    body.scrollTop = s;
  }

  function close() { if (el) to(0); }

  function startDrag(e) {
    if (e.button !== 0) return;
    anim?.stop();
    anim = null;
    drag = { y0: e.clientY, v0: vis, samples: [{ y: e.clientY, t: performance.now() }], id: e.pointerId };
    el.setPointerCapture?.(e.pointerId);
    el.classList.add("dragging");
    e.preventDefault();
  }

  document.addEventListener("pointermove", (e) => {
    if (!drag || e.pointerId !== drag.id) return;
    let v = drag.v0 - (e.clientY - drag.y0);
    const H = full();
    if (v > H) { const d = v - H; v = H + RUBBER * (1 - 1 / (d / RUBBER + 1)); } // rubber band
    vis = Math.max(0, v);
    const now = performance.now();
    drag.samples.push({ y: e.clientY, t: now });
    while (drag.samples.length > 2 && now - drag.samples[0].t > 90) drag.samples.shift();
    paint();
  });

  const endDrag = (e) => {
    if (!drag || (e && e.pointerId !== drag.id)) return;
    const s = drag.samples, a = s[0], b = s[s.length - 1];
    const v = b.t > a.t ? -(b.y - a.y) / (b.t - a.t) : 0; // px/ms, up = positive
    drag = null;
    el?.classList.remove("dragging");
    const A = anchors();
    let target;
    if (Math.abs(v) > 0.55) {
      // a throw: the next anchor in that direction
      target = v > 0 ? A.find((x) => x > vis + 4) ?? A[A.length - 1] : [...A].reverse().find((x) => x < vis - 4) ?? 0;
    } else {
      // where it would coast to, then the nearest anchor
      const proj = vis + v * 180;
      target = A.reduce((p, x) => (Math.abs(x - proj) < Math.abs(p - proj) ? x : p), A[0]);
    }
    to(target, v);
  };
  document.addEventListener("pointerup", endDrag);
  document.addEventListener("pointercancel", endDrag);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && el) { close(); e.stopPropagation(); e.preventDefault(); }
  }, true);

  return { open, update, close, isOpen: () => !!el, body: () => body, relayout: () => paint() };
})();

// ---------- card -> page (shared element) ----------
// The clicked card grows into the page: a copy of its background scales up to fill the page while
// the new page grows out of the card's spot and fades in.
function sharedZoom(fromRect, toEl) {
  if (motionOff() || !fromRect || !toEl) return;
  const to = toEl.getBoundingClientRect();
  if (!to.width || !to.height) return;
  const ghost = document.createElement("div");
  ghost.className = "zoom-ghost";
  Object.assign(ghost.style, { left: fromRect.left + "px", top: fromRect.top + "px", width: fromRect.width + "px", height: fromRect.height + "px" });
  document.body.append(ghost);
  const ease = getComputedStyle(document.documentElement).getPropertyValue("--ease-fluid").trim() || "cubic-bezier(0.2, 0.8, 0.2, 1)";
  const dx = fromRect.left + fromRect.width / 2 - (to.left + to.width / 2);
  const dy = fromRect.top + fromRect.height / 2 - (to.top + to.height / 2);
  ghost.animate([
    { left: fromRect.left + "px", top: fromRect.top + "px", width: fromRect.width + "px", height: fromRect.height + "px", borderRadius: "12px", opacity: 1 },
    { left: to.left + "px", top: to.top + "px", width: to.width + "px", height: to.height + "px", borderRadius: "16px", opacity: 1, offset: 0.75 },
    { left: to.left + "px", top: to.top + "px", width: to.width + "px", height: to.height + "px", borderRadius: "16px", opacity: 0 },
  ], { duration: 560, easing: ease, fill: "forwards" }).finished.finally(() => ghost.remove());
  toEl.animate([
    { transform: `translate(${dx}px, ${dy}px) scale(${Math.max(0.2, fromRect.width / to.width).toFixed(3)}, ${Math.max(0.1, fromRect.height / to.height).toFixed(3)})`, opacity: 0 },
    { opacity: 0, offset: 0.25 },
    { transform: "none", opacity: 1 },
  ], { duration: 560, easing: ease });
}

// ---------- staggered entrance ----------
// Items rise in one after another with a small spring. The page is rebuilt every second; within
// the entrance window the rebuilt items continue where they were (negative delay), not restart.
const Cascade = (() => {
  let t0 = 0, root = null;
  const SEL = "#list-inner > *, .cascade-items > *, .tiles > *, .hx-grid > *, .am-list > *, .set-page > *";
  function mark() {
    if (!root || performance.now() - t0 > 1200) { root?.classList.remove("cascade"); root = null; return; }
    const el = performance.now() - t0;
    root.style.setProperty("--el", el.toFixed(0) + "ms");
    let i = 0;
    for (const c of root.querySelectorAll(SEL)) c.style.setProperty("--ci", Math.min(14, i++));
  }
  function play(el) {
    root?.classList.remove("cascade");
    if (motionOff() || !el) { root = null; return; }
    root = el;
    t0 = performance.now();
    el.classList.remove("cascade");
    void el.offsetWidth;
    el.classList.add("cascade");
    mark();
    clearTimeout(play.t);
    play.t = setTimeout(() => { el.classList.remove("cascade"); if (root === el) root = null; }, 1300);
  }
  return { play, after: mark };
})();
