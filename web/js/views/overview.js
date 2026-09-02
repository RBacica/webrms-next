/* Overview — /api/reports/overview + movers + dept-weekly. */
"use strict";
export async function render(el, { API, SERVER }) {
  const ov = await API.get("/api/reports/overview" + (SERVER.branch ? `?branch=${SERVER.branch}` : ""));
  const s = ov.sales, st = ov.stock;
  const pct = (v) => v === null || v === undefined ? "—" : (v > 0 ? `+${v.toFixed(1)}%` : `${v.toFixed(1)}%`);
  el.innerHTML = `
  <div class="ov-summary">
    <div class="ov-line ov-cur"><b>Today ${fmt$(s.today)}</b> <span class="muted">as of ${ov.as_of}</span></div>
    <div class="ov-line">
      <span class="ov-seg">vs yesterday: <span class="${s.today_vs_yesterday_pct >= 0 ? "ok" : "bad"}">${pct(s.today_vs_yesterday_pct)}</span></span>
      <span class="ov-seg">vs last week: <span class="${s.today_vs_last_week_pct >= 0 ? "ok" : "bad"}">${pct(s.today_vs_last_week_pct)}</span></span>
      <span class="ov-seg">this week: <b>${fmt$(s.this_week)}</b></span>
      <span class="ov-seg">4wk avg: <b>${fmt$(s.four_wk_avg)}</b></span>
    </div>
    <div class="ov-line">
      <span class="ov-seg">stock items: <b>${st.items}</b></span>
      <span class="ov-seg">stock value: <b>${fmt$(st.value)}</b></span>
      <span class="ov-seg">stockouts: <span class="${st.stockout > 0 ? "bad" : "ok"}">${st.stockout}</span></span>
      <span class="ov-seg">scanback recd: <b class="ok">${fmt$(ov.scanback_received)}</b></span>
      <span class="ov-seg">GP incl scanback: <b>${ov.gp_incl_scanback.toFixed(1)}%</b></span>
    </div>
  </div>
  <div class="ov-grid">
    <div class="ov-block"><h3>Top movers (7d)</h3><div id="ov-movers">…</div></div>
    <div class="ov-block"><h3>Dept this week vs last (12wk avg)</h3><div id="ov-weekly">…</div></div>
  </div>`;

  const now = new Date();
  const to = toISO(now);
  const from = toISO(new Date(now.getTime() - 7 * 864e5));
  const [movers, weekly] = await Promise.all([
    API.get(`/api/reports/overview/movers?from=${from}&to=${to}${SERVER.branch ? `&branch=${SERVER.branch}` : ""}&limit=8`),
    API.get(`/api/reports/overview/dept-weekly${SERVER.branch ? `?branch=${SERVER.branch}` : ""}`),
  ]);
  el.querySelector("#ov-movers").innerHTML = table(
    ["#", "Item", "Units", "Net"],
    movers.map((m, i) => [i + 1, m.name, m.units, fmt$(m.net)]),
  );
  el.querySelector("#ov-weekly").innerHTML = table(
    ["Dept", "This wk", "Last wk", "Δ%", "12wk avg"],
    weekly.map((w) => [
      w.dept_name, fmt$(w.this_week_gross), fmt$(w.last_week_gross),
      w.delta_pct === null ? "—" : `${w.delta_pct.toFixed(0)}%`,
      w.avg_12wk === null ? "—" : fmt$(w.avg_12wk),
    ]),
    weekly.map((w) => (w.dept_id === "0" ? "ov-total-row" : "")),
  );
}

function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 0 }); }
function toISO(d) { return d.toISOString().slice(0, 10); }
function table(headers, rows, rowClasses = []) {
  return `<div class="table-wrap"><table><colgroup>${headers.map(() => "<col>").join("")}</colgroup>
    <thead><tr>${headers.map((h) => `<th>${h}</th>`).join("")}</tr></thead>
    <tbody>${rows.map((r, i) => `<tr class="${rowClasses[i] || ""}">${r.map((c) => `<td>${c}</td>`).join("")}</tr>`).join("")}</tbody>
  </table></div>`;
}
