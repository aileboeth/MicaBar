"use strict";
// Small SVG charts for the panel. Conventions: 2px lines with round joins, >=8px end dots with a
// 2px surface ring, 1px solid recessive gridlines, one y axis, text in ink colours (never the
// series colour), selective end labels, crosshair + tooltip on hover.

const Charts = (() => {
  const chartData = new Map(); // id -> { xs, series, geom, fmtX, fmtY, markers }
  const hover = new Map(); // id -> last mouse x inside the plot (CSS px), survives re-renders

  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

  // 1 / 2 / 2.5 / 5 x 10^n steps, 4 intervals. binary: pick the KB/MB/GB unit first and round
  // inside it, so byte axes read 0 / 1 MB / 2 MB rather than 0 / 977 KB / 1.9 MB.
  function niceScale(max, ticks = 4, binary = false) {
    if (!(max > 0)) return { max: 1, step: 0.25 };
    let unit = 1;
    if (binary) while (max / unit >= 1024) unit *= 1024;
    const m = max / unit;
    const raw = m / ticks;
    const mag = Math.pow(10, Math.floor(Math.log10(raw)));
    const step = [1, 2, 2.5, 5, 10].map((x) => x * mag).find((s) => s >= raw) || 10 * mag;
    return { max: step * ticks * unit, step: step * unit };
  }

  /**
   * Line chart.
   * opts: { id, width, height, xs: number[], series: [{ name, cls, values: (number|null)[] }],
   *         fmtY(v), fmtTick(v), fmtX(x), endLabels: bool, ref: { value, label },
   *         markers: [{ i, label }] (critical marks, drawn at the top) }
   */
  function line(opts) {
    const { id, width: W, height: H, xs, series } = opts;
    const pad = { l: 46, r: opts.endLabels ? 70 : 12, t: 10, b: 20 };
    const pw = W - pad.l - pad.r, ph = H - pad.t - pad.b;
    const n = xs.length;
    let max = 0;
    for (const s of series) for (const v of s.values) if (v != null && v > max) max = v;
    if (opts.ref && opts.ref.value > max) max = opts.ref.value;
    const sc = niceScale(max * 1.05, 4, !!opts.binary);
    const X = (i) => pad.l + (n <= 1 ? pw : (i / (n - 1)) * pw);
    const Y = (v) => pad.t + ph - (v / sc.max) * ph;
    chartData.set(id, { xs, series, X, Y, pad, W, H, n, fmtX: opts.fmtX, fmtY: opts.fmtY, markers: opts.markers || [] });

    let grid = "";
    for (let v = 0; v <= sc.max + 1e-9; v += sc.step) {
      const y = Y(v).toFixed(1);
      grid += `<line class="c-grid" x1="${pad.l}" x2="${pad.l + pw}" y1="${y}" y2="${y}"/>`;
      grid += `<text class="c-tick" x="${pad.l - 6}" y="${y}" text-anchor="end" dominant-baseline="middle">${esc(opts.fmtTick ? opts.fmtTick(v) : v)}</text>`;
    }
    // x labels: first and last only (the tooltip carries the rest)
    let xl = "";
    if (n > 1 && opts.fmtX) {
      xl += `<text class="c-tick" x="${pad.l}" y="${H - 4}" text-anchor="start">${esc(opts.fmtX(xs[0]))}</text>`;
      xl += `<text class="c-tick" x="${pad.l + pw}" y="${H - 4}" text-anchor="end">${esc(opts.fmtX(xs[n - 1]))}</text>`;
    }
    let ref = "";
    if (opts.ref) {
      const y = Y(opts.ref.value).toFixed(1);
      ref = `<line class="c-ref" x1="${pad.l}" x2="${pad.l + pw}" y1="${y}" y2="${y}"/><text class="c-tick" x="${pad.l + pw}" y="${(+y - 4).toFixed(1)}" text-anchor="end">${esc(opts.ref.label)}</text>`;
    }
    let paths = "", ends = "";
    const endPts = [];
    for (const s of series) {
      let d = "", pen = false;
      s.values.forEach((v, i) => {
        if (v == null) { pen = false; return; }
        d += `${pen ? "L" : "M"}${X(i).toFixed(1)},${Y(v).toFixed(1)}`;
        pen = true;
      });
      paths += `<path class="c-line ${s.cls}" d="${d}"/>`;
      let li = s.values.length - 1;
      while (li >= 0 && s.values[li] == null) li--;
      if (li >= 0) {
        const ex = X(li), ey = Y(s.values[li]);
        ends += `<circle class="c-dot ${s.cls}" cx="${ex.toFixed(1)}" cy="${ey.toFixed(1)}" r="4"/>`;
        endPts.push({ y: ey, x: ex, s, v: s.values[li] });
      }
    }
    // end labels only when they don't collide; otherwise legend + tooltip carry identity
    let labels = "";
    if (opts.endLabels && endPts.length) {
      const sorted = [...endPts].sort((a, b) => a.y - b.y);
      const clash = sorted.some((p, i) => i && p.y - sorted[i - 1].y < 26);
      if (!clash) {
        for (const p of endPts) {
          labels += `<text class="c-end" x="${(p.x + 8).toFixed(1)}" y="${(p.y - 3).toFixed(1)}">${esc(opts.fmtY(p.v))}</text>`;
          labels += `<text class="c-endname" x="${(p.x + 8).toFixed(1)}" y="${(p.y + 10).toFixed(1)}">${esc(p.s.name)}</text>`;
        }
      }
    }
    let marks = "";
    for (const m of opts.markers || []) {
      const x = X(m.i).toFixed(1);
      marks += `<g class="c-crit"><line x1="${x}" x2="${x}" y1="${pad.t}" y2="${pad.t + ph}"/><text x="${x}" y="${pad.t + 8}" text-anchor="middle">✕</text></g>`;
    }
    return `<div class="chart" data-chart="${esc(id)}" style="width:${W}px;height:${H}px">
      <svg width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">${grid}${xl}${ref}${marks}${paths}${ends}${labels}
        <g class="c-hover" style="display:none"><line class="c-cross" y1="${pad.t}" y2="${pad.t + ph}"/>${series.map((s) => `<circle class="c-hdot ${s.cls}" r="4"/>`).join("")}</g>
        <rect class="c-hit" x="${pad.l}" y="0" width="${pw}" height="${H}" fill="transparent"/>
      </svg><div class="c-tip" style="display:none"></div></div>`;
  }

  function showHover(el, mx) {
    const id = el.dataset.chart;
    const d = chartData.get(id);
    if (!d || d.n === 0) return;
    const t = (mx - d.pad.l) / (d.W - d.pad.l - d.pad.r);
    const i = Math.max(0, Math.min(d.n - 1, Math.round(t * (d.n - 1))));
    const g = el.querySelector(".c-hover");
    const tip = el.querySelector(".c-tip");
    if (!g || !tip) return;
    const x = d.X(i);
    g.style.display = "";
    const cross = g.querySelector(".c-cross");
    cross.setAttribute("x1", x);
    cross.setAttribute("x2", x);
    const dots = g.querySelectorAll(".c-hdot");
    let rows = "";
    d.series.forEach((s, k) => {
      const v = s.values[i];
      if (v == null) {
        dots[k].style.display = "none";
      } else {
        dots[k].style.display = "";
        dots[k].setAttribute("cx", x);
        dots[k].setAttribute("cy", d.Y(v));
      }
      rows += `<div class="tip-row"><span class="tip-key ${s.cls}"></span>${esc(s.name)}<b>${v == null ? "—" : esc(d.fmtY(v))}</b></div>`;
    });
    const mk = d.markers.find((m) => m.i === i);
    if (mk) rows += `<div class="tip-row tip-crit">✕ ${esc(mk.label)}</div>`;
    tip.innerHTML = `<div class="tip-head">${esc(d.fmtX ? d.fmtX(d.xs[i]) : i)}</div>${rows}`;
    tip.style.display = "";
    const left = x + 12 + tip.offsetWidth > d.W ? x - 12 - tip.offsetWidth : x + 12;
    tip.style.left = left + "px";
    tip.style.top = d.pad.t + "px";
  }

  function hideHover(el) {
    const g = el.querySelector(".c-hover");
    const tip = el.querySelector(".c-tip");
    if (g) g.style.display = "none";
    if (tip) tip.style.display = "none";
  }

  // Event delegation on a container; hover survives the container being re-rendered.
  function attach(container) {
    container.addEventListener("mousemove", (e) => {
      const el = e.target.closest(".chart");
      if (!el || !e.target.closest(".c-hit, .c-hover, .c-tip")) return;
      const r = el.getBoundingClientRect();
      const mx = e.clientX - r.left;
      hover.set(el.dataset.chart, mx);
      showHover(el, mx);
    });
    container.addEventListener("mouseleave", () => hover.clear());
    container.addEventListener("mouseout", (e) => {
      const el = e.target.closest(".chart");
      if (el && !el.contains(e.relatedTarget)) {
        hover.delete(el.dataset.chart);
        hideHover(el);
      }
    });
  }

  // Call after a re-render to restore an active hover.
  function restore(container) {
    for (const [id, mx] of hover) {
      const el = container.querySelector(`.chart[data-chart="${CSS.escape(id)}"]`);
      if (el) showHover(el, mx);
    }
  }

  /**
   * Stacked columns. opts: { id, width, height, cats: string[], stacks: [{ name, cls, values }],
   * fmtY, fmtTick, fmtCat }. Columns <= 24px, 2px surface gap between segments, 4px rounded top
   * on the top segment, square at the baseline, total on the cap.
   */
  function columns(opts) {
    const { id, width: W, height: H, cats, stacks } = opts;
    const pad = { l: 46, r: 10, t: 18, b: 20 };
    const pw = W - pad.l - pad.r, ph = H - pad.t - pad.b;
    const totals = cats.map((_, i) => stacks.reduce((s, st) => s + (st.values[i] || 0), 0));
    const sc = niceScale(Math.max(...totals, 0) * 1.05, 4, !!opts.binary);
    const Y = (v) => pad.t + ph - (v / sc.max) * ph;
    const band = pw / Math.max(1, cats.length);
    const bw = Math.min(24, band * 0.6);
    let grid = "";
    for (let v = 0; v <= sc.max + 1e-9; v += sc.step) {
      const y = Y(v).toFixed(1);
      grid += `<line class="c-grid" x1="${pad.l}" x2="${pad.l + pw}" y1="${y}" y2="${y}"/>`;
      grid += `<text class="c-tick" x="${pad.l - 6}" y="${y}" text-anchor="end" dominant-baseline="middle">${esc(opts.fmtTick(v))}</text>`;
    }
    let bars = "";
    // many columns (30 days): every k-th label, totals on the caps only when there is room
    const tickEvery = Math.max(1, Math.ceil(40 / band));
    const caps = band >= 34;
    const selIdx = opts.selected != null ? cats.indexOf(opts.selected) : -1;
    cats.forEach((c, i) => {
      const cx = pad.l + band * i + band / 2;
      const x = cx - bw / 2;
      const dim = opts.selected != null && opts.selected !== c ? " dim" : "";
      const hit = !opts.pick ? "" : `<rect class="c-hit" x="${pad.l + band * i}" y="${pad.t}" width="${band}" height="${ph}" data-${opts.pick}="${esc(c)}"><title>${esc(opts.fmtCat(c))} · 合计 ${esc(opts.fmtY(totals[i]))}${stacks.map((st) => ` · ${esc(st.name)} ${esc(opts.fmtY(st.values[i] || 0))}`).join("")}</title></rect>`;
      let base = 0;
      const segs = stacks.map((st) => ({ st, v: st.values[i] || 0 })).filter((s) => s.v > 0);
      segs.forEach((s, k) => {
        const y0 = Y(base), y1 = Y(base + s.v);
        base += s.v;
        const gap = k > 0 ? 2 : 0; // surface gap between stacked segments
        const top = y1, h = Math.max(0, y0 - y1 - gap);
        if (h <= 0) return;
        const r = k === segs.length - 1 ? Math.min(4, h) : 0;
        // square bottom, rounded top only for the top segment
        const p = r
          ? `M${x},${top + h} V${top + r} Q${x},${top} ${x + r},${top} H${x + bw - r} Q${x + bw},${top} ${x + bw},${top + r} V${top + h} Z`
          : `M${x},${top + h} V${top} H${x + bw} V${top + h} Z`;
        bars += `<path class="c-bar ${s.st.cls}${dim}" d="${p}"><title>${esc(opts.fmtCat(c))} · ${esc(s.st.name)} ${esc(opts.fmtY(s.v))}</title></path>`;
      });
      const sel = opts.selected === c;
      if (totals[i] > 0 && (caps || sel)) bars += `<text class="c-cap${sel ? " sel" : ""}" x="${cx}" y="${(Y(totals[i]) - 5).toFixed(1)}" text-anchor="middle">${esc(opts.fmtTick(totals[i]))}</text>`;
      const nearSel = selIdx >= 0 && !sel && Math.abs(i - selIdx) < tickEvery;
      if (sel || (!nearSel && (cats.length - 1 - i) % tickEvery === 0)) bars += `<text class="c-tick${sel ? " sel" : ""}" x="${cx}" y="${H - 4}" text-anchor="middle">${esc(opts.fmtCat(c))}</text>`;
      bars += hit; // on top, so a click anywhere in the column picks it
    });
    return `<div class="chart" data-chart="${esc(id)}" style="width:${W}px;height:${H}px"><svg width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">${grid}${bars}</svg></div>`;
  }

  /** Thin segmented bar (8px, rounded ends), parts proportional; zero parts draw nothing. */
  function segbar(parts, title = "") {
    const total = parts.reduce((s, p) => s + p.value, 0);
    if (!(total > 0)) return `<div class="segbar empty"></div>`;
    return `<div class="segbar" title="${esc(title)}">${parts
      .filter((p) => p.value > 0)
      .map((p) => `<span class="seg ${p.cls}" style="flex:${p.value}"></span>`)
      .join("")}</div>`;
  }

  /** Sparkline: one series in the de-emphasis hue, no axes. */
  function spark(values, w = 90, h = 22) {
    const max = Math.max(...values, 0);
    if (!(max > 0) || values.length < 2) return `<svg class="spark" width="${w}" height="${h}"></svg>`;
    const X = (i) => (i / (values.length - 1)) * (w - 2) + 1;
    const Y = (v) => h - 2 - (v / max) * (h - 4);
    const d = values.map((v, i) => `${i ? "L" : "M"}${X(i).toFixed(1)},${Y(v).toFixed(1)}`).join("");
    return `<svg class="spark" width="${w}" height="${h}"><path d="${d}"/></svg>`;
  }

  return { line, columns, segbar, spark, attach, restore };
})();
