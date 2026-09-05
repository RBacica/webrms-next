/* Overview — full landing matching original WebRMS: summary comparisons,
   basket composition by dept, top movers (expandable), dept weekly, weekday
   curve, peak hours + payment mix, basket-size bands, promo uplifts,
   top movers by dept. All helpers self-contained (ES module). */
"use strict";

export async function render(el, { API, SERVER }) {
  const branchQ = () => (SERVER.branch ? `?branch=${SERVER.branch}` : "");
  const now = new Date();
  const to = iso(now);
  const from = iso(new Date(now.getTime() - 6 * 864e5));
  const monthFrom = iso(new Date(now.getTime() - 29 * 864e5));

  const ov = await API.get("/api/reports/overview" + branchQ());
  el.innerHTML = overviewHtml(ov);

  // async slices fill after first paint
  const p = branchQ();
  API.get(`/api/reports/overview/movers?from=${from}&to=${to}${p}&limit=8`)
    .then((m) => { fill(el, "ov-movers-body", moversHtml(m)); bindCaret(el); })
    .catch((e) => fill(el, "ov-movers-body", `<tr><td colspan="5" class="muted">${esc(e.message)}</td></tr>`));
  API.get(`/api/reports/overview/dept-weekly${p}`)
    .then((dw) => fill(el, "ov-dept-weekly-body", deptWeeklyHtml(dw)))
    .catch((e) => fill(el, "ov-dept-weekly-body", `<tr><td colspan="6" class="muted">${esc(e.message)}</td></tr>`));
  API.get(`/api/reports/overview/dept-movers?from=${from}&to=${to}${p}`)
    .then((dm) => { fill(el, "ov-dept-movers-body", deptMoversHtml(dm)); bindDeptCaret(el); })
    .catch((e) => fill(el, "ov-dept-movers-body", `<tr><td colspan="4" class="muted">${esc(e.message)}</td></tr>`));
  API.get(`/api/reports/promo-summary?from=${monthFrom}&to=${to}`)
    .then((ps) => fill(el, "ov-promos", promosHtml(ps)))
    .catch((e) => fill(el, "ov-promos", `<span class="muted">Promo insights unavailable: ${esc(e.message)}</span>`));
}

function overviewHtml(ov) {
  const s = ov.sales || {};
  const st = ov.stock || {};
  const b = ov.basket || {};
  const vt = ov.voids_today || {};

  // Line 1 — today
  const dailyLine = `<div class="ov-line">
    <span class="ov-seg ov-cur"><b>Today</b> <b title="$${s.today?.toFixed(2) ?? "—"}">${fmt$(s.today ?? 0)}</b></span>
    <span class="ov-seg"><b class="muted">vs LastWeek (Today)</b> <b title="$${(s.last_week_same_day ?? 0).toFixed(2)}">${fmt$(s.last_week_same_day ?? 0)}</b>${pctSpan(s.today_vs_last_week_pct)}</span>
    <span class="ov-seg"><b class="muted">vs WeeklyAvg</b> <b class="muted">(4-wk)</b> <b title="$${(s.four_wk_avg ?? 0).toFixed(2)}">${fmt$(s.four_wk_avg ?? 0)}</b></span>
  </div>`;

  // Line 2 — this week
  const weeklyLine = `<div class="ov-line">
    <span class="ov-seg ov-cur"><b>CurrentWeek</b> <b title="$${(s.this_week ?? 0).toFixed(2)}">${fmt$(s.this_week ?? 0)}</b></span>
    <span class="ov-seg"><b class="muted">vs LastWeek</b> <b title="$${(s.last_week ?? 0).toFixed(2)}">${fmt$(s.last_week ?? 0)}</b></span>
    <span class="ov-seg muted">as of ${ov.as_of}</span>
  </div>`;

  // Line 3 — the rest
  const statsLine = `<div class="ov-line">
    <span class="ov-seg muted">${fmt$(s.today ?? 0)} gross · <b>Voids</b> ${vt.count ?? 0} (${fmt$(vt.value ?? 0)})</span>
    <span class="ov-seg"><b>Stock</b> ${fmt$(st.retail_value ?? 0)} retail / GP ${fmt$(st.gp_value ?? 0)}</span>
    <span class="ov-seg"><b>Avg basket</b> ${fmt$(b.week7_avg ?? 0)} (7d) · ${(b.items_per_basket ?? 0).toFixed(1)} items/basket</span>
    <span class="ov-seg"><b class="${(st.stockout ?? 0) > 0 ? "bad" : "ok"}">${st.stockout ?? 0} stock-outs</b> · ${st.low_stock ?? 0} low</span>
    <span class="ov-seg">GP incl scanback: <b>${(ov.gp_incl_scanback ?? 0).toFixed(1)}%</b></span>
  </div>`;

  const deptRows = (b.depts || []).map((x) => `<tr>
    <td>${esc(x.dept_name)}</td>
    <td class="num">${fmt$(x.avg_net_per_basket)}</td>
    <td class="num">${x.avg_units_per_basket.toFixed(2)}</td>
    <td class="num">${x.share_pct.toFixed(1)}%</td>
    <td>${shareBar(x.share_pct)}</td>
  </tr>`).join("") || `<tr><td colspan="5" class="muted">No sales in window.</td></tr>`;

  const distRows = (b.dist || []).map((x) => `<tr>
    <td>${esc(x.band)}</td><td class="num">${x.txns}</td><td class="num">${x.share_pct.toFixed(1)}%</td><td>${shareBar(x.share_pct)}</td>
  </tr>`).join("") || `<tr><td colspan="4" class="muted">No data.</td></tr>`;

  const wd = b.weekday || [];
  const maxAvg = wd.length ? Math.max(...wd.map((w) => w.avg_net)) : 0;
  const minAvg = wd.length ? Math.min(...wd.map((w) => w.avg_net)) : 0;
  const wdCells = OV_DAYS.map((lbl, i) => {
    const w = wd.find((x) => x.dow === i);
    const cls = (w && maxAvg > minAvg && w.avg_net === maxAvg) ? "best" : (w && maxAvg > minAvg && w.avg_net === minAvg ? "worst" : "");
    return `<div class="wd ${cls}"><div class="lbl">${lbl}</div><div class="val">${w ? fmt$(w.avg_net) : "—"}</div><div class="lbl">${w ? w.txns + " txns" : ""}</div></div>`;
  }).join("");

  const peaks = (b.peak_hours || []).map((p) => `<span class="ov-chip">${String(p.hour).padStart(2, "0")}:00–${String(p.hour + 1).padStart(2, "0")}:00 ${fmt$(p.net)}</span>`).join("") || `<span class="muted">—</span>`;
  const pays = (b.payments || []).map((p) => `<span class="ov-chip">${esc(p.media)} ${p.share_pct.toFixed(1)}%</span>`).join("") || `<span class="muted">—</span>`;

  return `<div class="ov">
    <div class="ov-summary">${dailyLine}${weeklyLine}${statsLine}</div>
    <div class="ov-grid">
      <div class="ov-block">
        <h3>Basket composition by department · 7 days</h3>
        <table><thead><tr><th>Department</th><th class="num">Avg $/basket</th><th class="num">Units</th><th class="num">Share</th><th></th></tr></thead>
        <tbody>${deptRows}</tbody></table>
      </div>
      <div class="ov-block">
        <h3>Top 5 movers · 7 days</h3>
        <table><thead><tr><th></th><th>Product</th><th>Dept</th><th class="num">Units</th><th class="num">Net</th></tr></thead>
        <tbody id="ov-movers-body"><tr><td colspan="5" class="muted">Loading movers…</td></tr></tbody></table>
      </div>
    </div>
    <div class="ov-block" style="margin-bottom:14px">
      <h3>This week vs last week by department</h3>
      <table><thead><tr><th>Department</th><th class="num">This week</th><th class="num">Last week</th><th class="num">% Δ</th><th class="num">12-wk avg</th><th class="num">% Δ vs avg</th></tr></thead>
      <tbody id="ov-dept-weekly-body"><tr><td colspan="6" class="muted">Loading…</td></tr></tbody></table>
    </div>
    <div class="ov-grid">
      <div class="ov-block">
        <h3>Average basket by weekday · 28 days</h3>
        <div class="ov-weekday">${wdCells}</div>
      </div>
      <div class="ov-block">
        <h3>Peak hours &amp; payment mix · 7 days</h3>
        <div style="margin-bottom:6px"><b class="muted">Peak:</b> ${peaks}</div>
        <div><b class="muted">Payments:</b> ${pays}</div>
      </div>
    </div>
    <div class="ov-grid">
      <div class="ov-block">
        <h3>Basket size · 7 days</h3>
        <table><thead><tr><th>Band</th><th class="num">Baskets</th><th class="num">Share</th><th></th></tr></thead>
        <tbody>${distRows}</tbody></table>
      </div>
      <div class="ov-block">
        <h3>Top promo uplifts · 30 days</h3>
        <div id="ov-promos"><span class="muted">Measuring promo effectiveness…</span></div>
      </div>
    </div>
    <div class="ov-block">
      <h3>Top movers by department · 7 days</h3>
      <table><thead><tr><th>Dept</th><th class="num" colspan="3">Top products by net</th></tr></thead>
      <tbody id="ov-dept-movers-body"><tr><td colspan="4" class="muted">Loading…</td></tr></tbody></table>
    </div>
  </div>`;
}

const OV_DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function pctSpan(p) {
  if (p === null || p === undefined) return "";
  const cls = p < 0 ? "bad" : "ok";
  const arrow = p > 0 ? "▲" : "▼";
  return `<span class="${cls}"> ${arrow} ${Math.abs(p).toFixed(1)}%</span>`;
}

function shareBar(pct) {
  return `<div class="ov-bar-wrap"><div class="ov-bar" style="width:${Math.min(100, pct).toFixed(1)}%"></div></div>`;
}

function moversHtml(list) {
  const rows = (list || []).map((m, i) => {
    const hasKids = m.children && m.children.length > 0;
    const kids = hasKids ? m.children.map((c) => `<tr class="ov-mover-child" data-p="${i}" style="display:none">
      <td></td><td><span class="ov-child-dot">·</span><span class="muted">${esc(c.upc)}</span> · ${esc(c.name)}</td>
      <td class="muted">${esc(m.dept || "")}</td><td class="num">${c.units.toFixed(0)}</td><td class="num">${fmt$(c.net)}</td>
    </tr>`).join("") : "";
    const caret = hasKids ? `<span class="ov-mover-caret" data-p="${i}">▶</span>` : "";
    return `<tr class="ov-mover-row${hasKids ? " ov-mover-hdr" : ""}${i % 2 ? " ov-mover-hdr-alt" : ""}" ${hasKids ? `data-p="${i}" style="cursor:pointer"` : ""}>
      <td class="num muted">${i + 1} ${caret}</td>
      <td>${esc(m.name)}</td><td class="muted">${esc(m.dept || "")}</td>
      <td class="num">${m.units.toFixed(0)}</td><td class="num">${fmt$(m.net)}</td>
    </tr>${kids}`;
  }).join("") || `<tr><td colspan="5" class="muted">No sales in window.</td></tr>`;
  return rows;
}

function deptWeeklyHtml(list) {
  return (list || []).map((w) => `<tr class="${w.dept_id === "0" ? "ov-total-row" : ""}">
    <td>${esc(w.dept_name)}</td>
    <td class="num">${fmt$(w.this_week_gross)}</td>
    <td class="num">${fmt$(w.last_week_gross)}</td>
    <td class="num">${w.delta_pct === null ? "—" : w.delta_pct.toFixed(1) + "%"}</td>
    <td class="num">${w.avg_12wk === null ? "—" : fmt$(w.avg_12wk)}</td>
    <td class="num">${w.avg_pct === null ? "—" : w.avg_pct.toFixed(1) + "%"}</td>
  </tr>`).join("") || `<tr><td colspan="6" class="muted">No data.</td></tr>`;
}

function deptMoversHtml(list) {
  return (list || []).map((d, di) => {
    const inner = (d.movers || []).map((m, mi) => {
      const hasKids = m.children && m.children.length > 0;
      const kids = hasKids ? m.children.map((c) => `<tr class="ov-dm-mover-child" data-d="${di}" data-m="${mi}" style="display:none">
        <td></td><td><span class="ov-child-dot">·</span><span class="muted">${esc(c.upc)}</span> · ${esc(c.name)}</td>
        <td class="num">${c.units.toFixed(0)}</td><td class="num">${fmt$(c.net)}</td>
      </tr>`).join("") : "";
      const caret = hasKids ? `<span class="ov-dm-mover-caret" data-d="${di}" data-m="${mi}">▶</span>` : "";
      return `<tr class="ov-dm-row" ${hasKids ? `data-d="${di}" data-m="${mi}" style="cursor:pointer;display:none"` : 'style="display:none"'}>
        <td class="num muted">${mi + 1} ${caret}</td>
        <td>${esc(m.name)}</td><td class="num">${m.units.toFixed(0)}</td><td class="num">${fmt$(m.net)}</td>
      </tr>${kids}`;
    }).join("");
    return `<tr class="ov-dm-hdr${di % 2 ? " ov-dm-hdr-alt" : ""}" data-d="${di}" style="cursor:pointer">
      <td><span class="ov-dm-caret" data-d="${di}">▶</span> <b>${esc(d.dept_name)}</b></td>
      <td colspan="3" class="muted">${(d.movers || []).length} movers</td>
    </tr>${inner}`;
  }).join("") || `<tr><td colspan="4" class="muted">No data.</td></tr>`;
}

function promosHtml(ps) {
  if (!ps || !ps.top || !ps.top.length) return `<span class="muted">No measured promos in 30d.</span>`;
  return `<p class="muted">${ps.measured} promos measured</p>` +
    ps.top.map((t) => `<div style="margin:2px 0"><b>${esc(t.description)}</b> <span class="muted">· ${t.units.toFixed(0)} units</span> <b class="num">${fmt$(t.net)}</b></div>`).join("");
}

function bindCaret(el) {
  el.querySelectorAll(".ov-mover-hdr").forEach((row) => {
    row.onclick = () => {
      const p = row.dataset.p;
      const kids = el.querySelectorAll(`.ov-mover-child[data-p="${p}"]`);
      const caret = row.querySelector(".ov-mover-caret");
      kids.forEach((k) => { k.style.display = k.style.display === "none" ? "" : "none"; });
      if (caret) caret.textContent = caret.textContent === "▶" ? "▼" : "▶";
    };
  });
}

function bindDeptCaret(el) {
  el.querySelectorAll(".ov-dm-hdr").forEach((row) => {
    row.onclick = () => {
      const d = row.dataset.d;
      const rows = el.querySelectorAll(`.ov-dm-row[data-d="${d}"]`);
      const caret = row.querySelector(".ov-dm-caret");
      const show = caret.textContent === "▶";
      rows.forEach((r) => { r.style.display = show ? "" : "none"; });
      caret.textContent = show ? "▼" : "▶";
    };
  });
}

function fill(el, id, html) {
  const node = el.querySelector("#" + id);
  if (node) node.innerHTML = html;
}

function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 0 }); }
function iso(d) { return d.toISOString().slice(0, 10); }
function esc(s) { const d = document.createElement("div"); d.textContent = s == null ? "" : s; return d.innerHTML; }
