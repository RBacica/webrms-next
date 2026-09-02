/* Reports — dept sales breakdown + daily summary. */
"use strict";
export async function render(el, { API, SERVER }) {
  const now = new Date();
  const iso = (d) => d.toISOString().slice(0, 10);
  const to = iso(now);
  const from = iso(new Date(now.getTime() - 30 * 864e5));
  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>From</label><input type="date" id="rp-from" value="${from}">
      <label>To</label><input type="date" id="rp-to" value="${to}">
      <div class="btn-group">
        <button id="rp-depts" class="active">Dept sales</button>
        <button id="rp-daily" class="secondary">Daily</button>
      </div>
    </div>
    <div id="rp-msg" class="msg"></div>
  </div>
  <div id="rp-results" class="panel"><div class="placeholder"><h2>Loading…</h2></div></div>`;

  const $ = (id) => el.querySelector(id);
  const q = (SERVER.branch ? `&branch=${SERVER.branch}` : "");

  async function depts() {
    try {
      const d = await API.get(`/api/reports/depts?from=${$("rp-from").value}&to=${$("rp-to").value}${q}`);
      $("rp-results").innerHTML = d.map((dept) => `
        <div class="panel" style="margin-bottom:10px">
          <h3>${esc(dept.dept_name)} <span class="muted">· target ${dept.target_margin}%</span>
            <span class="muted">· ${fmt$(dept.net)} net / ${fmt$(dept.cost)} cost</span></h3>
          <div class="table-wrap"><table><colgroup><col><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
          <thead><tr><th>Product</th><th class="num">Units</th><th class="num">Net</th><th class="num">Margin</th></tr></thead>
          <tbody>${dept.products.slice(0, 30).map((p) => `<tr>
            <td>${esc(p.name)}<div class="muted">${p.upc}</div></td>
            <td class="num">${p.units}</td><td class="num">${fmt$(p.net)}</td><td class="num">${fmt$(p.margin_amt)}</td>
          </tr>`).join("")}</tbody></table></div>
        </div>`).join("") || '<div class="placeholder"><h2>No sales in range</h2></div>';
    } catch (e) { $("rp-msg").className = "msg error"; $("rp-msg").textContent = e.message; }
  }
  async function daily() {
    try {
      const d = await API.get(`/api/reports/daily?from=${$("rp-from").value}&to=${$("rp-to").value}${q}`);
      $("rp-results").innerHTML = `<div class="table-wrap"><table><colgroup><col><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
        <thead><tr><th>Date</th><th class="num">Txns</th><th class="num">Gross</th><th class="num">Cost</th></tr></thead>
        <tbody>${d.daily.map((r) => `<tr><td>${esc(r.date)}</td><td class="num">${r.txns}</td><td class="num">${fmt$(r.gross_total)}</td><td class="num">${fmt$(r.cost)}</td></tr>`).join("")}
        <tr class="ov-total-row"><td>${d.totals.date}</td><td class="num">${d.totals.txns}</td><td class="num">${fmt$(d.totals.gross_total)}</td><td class="num">${fmt$(d.totals.cost)}</td></tr>
        </tbody></table></div>`;
    } catch (e) { $("rp-msg").className = "msg error"; $("rp-msg").textContent = e.message; }
  }
  $("rp-depts").onclick = () => { $("rp-depts").classList.add("active"); $("rp-daily").classList.remove("active"); depts(); };
  $("rp-daily").onclick = () => { $("rp-daily").classList.add("active"); $("rp-depts").classList.remove("active"); daily(); };
  depts();
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 0 }); }
