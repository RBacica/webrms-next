/* Promotions — active list (both engines), item drill-down, effectiveness. */
"use strict";
export async function render(el, { API, SERVER }) {
  const d = await API.get("/api/promotions/list" + (SERVER.branch ? `?branch=${SERVER.branch}` : ""));
  el.innerHTML = `
  <div class="panel">
    <div class="stats-row">
      <div class="stat"><span class="stat-val">${d.active}</span><span class="stat-lbl">Active</span></div>
      <div class="stat"><span class="stat-val">${d.inactive}</span><span class="stat-lbl">Inactive</span></div>
      <div class="stat"><span class="stat-val">${d.engine}</span><span class="stat-lbl">Engine</span></div>
    </div>
    <div class="toolbar">
      <button id="pr-list" class="toggle-btn active">Promotions</button>
      <button id="pr-eff" class="toggle-btn">Effectiveness</button>
    </div>
    <div id="pr-msg" class="msg"></div>
  </div>
  <div id="pr-results" class="panel"><div class="placeholder"><h2>Loading…</h2></div></div>`;

  const $ = (id) => el.querySelector(id.startsWith("#") ? id : "#" + id);
  renderList();

  function renderList() {
    $("pr-results").innerHTML = `<div class="table-wrap"><table><colgroup><col><col><col class="c-num"><col><col></colgroup>
      <thead><tr><th>Description</th><th>Scope</th><th class="num">Price</th><th>Window</th><th></th></tr></thead>
      <tbody>${d.promotions.map((p) => `<tr>
        <td>${esc(p.description || p.product)}<div class="muted">${p.id}</div></td>
        <td>${p.scope}</td><td class="num">${fmt$(p.price)}</td>
        <td>${esc(p.from_date || "")} → ${esc(p.to_date || "")}</td>
        <td><button class="secondary" data-id="${p.id}" data-name="${esc(p.description || p.product)}" onclick="window.__prItems(this)">Items</button></td>
      </tr>`).join("")}</tbody></table></div>`;
  }

  window.__prItems = async (btn) => {
    try {
      const it = await API.get(`/api/promotions/items?id=${btn.dataset.id}${SERVER.branch ? `&branch=${SERVER.branch}` : ""}`);
      let html = `<h3>${btn.dataset.name} — ${it.items.length} items</h3>`;
      if (it.deal) {
        html += `<p class="muted">${it.deal.deal_type}${it.deal.deal_price ? ` @ ${fmt$(it.deal.deal_price)}` : ""}${it.deal.discount_pct ? ` ${it.deal.discount_pct}% off` : ""}</p>`;
      }
      html += `<div class="table-wrap"><table><colgroup><col><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
        <thead><tr><th>Item</th><th class="num">Avg cost</th><th class="num">Disc%</th><th class="num">GP%</th></tr></thead>
        <tbody>${it.items.map((i) => `<tr><td>${esc(i.description)}<div class="muted">${i.upc} · ${i.cost_source}</div></td>
          <td class="num">${fmt$(i.avg_cost)}</td><td class="num">${i.discount_pct === null ? "—" : i.discount_pct.toFixed(1) + "%"}</td>
          <td class="num">${i.gp_pct === null ? "—" : i.gp_pct.toFixed(1) + "%"}</td></tr>`).join("")}</tbody></table></div>`;
      $("pr-results").innerHTML = `<div class="panel" style="margin:0">${html}</div>`;
      $("pr-msg").className = "msg";
      $("pr-msg").textContent = "Back to list: click Promotions.";
    } catch (e) { $("pr-msg").className = "msg error"; $("pr-msg").textContent = e.message; }
  };

  $("pr-eff").onclick = async () => {
    $("pr-eff").classList.add("active"); $("pr-list").classList.remove("active");
    const now = new Date();
    const to = now.toISOString().slice(0, 10);
    const from = new Date(now.getTime() - 30 * 864e5).toISOString().slice(0, 10);
    try {
      const r = await API.get(`/api/promotions/effectiveness?from=${from}&to=${to}${SERVER.branch ? `&branch=${SERVER.branch}` : ""}`);
      const rows = r.specials || [];
      $("pr-results").innerHTML = `<div class="stats-row"><div class="stat"><span class="stat-val">${rows.length}</span><span class="stat-lbl">Measured</span></div></div>
        <div class="table-wrap"><table><colgroup><col><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
        <thead><tr><th>Promotion</th><th class="num">Promo units</th><th class="num">Base units</th><th class="num">Uplift</th><th class="num">Promo net</th></tr></thead>
        <tbody>${rows.slice(0, 100).map((s) => `<tr><td>${esc(s.description || s.upc)}<div class="muted">${s.upc}</div></td>
          <td class="num">${s.promo_units}</td><td class="num">${s.base_units}</td>
          <td class="num">${s.uplift_units === null ? "—" : s.uplift_units.toFixed(2) + "×"}</td>
          <td class="num">${fmt$(s.promo_net)}</td></tr>`).join("")}</tbody></table></div>`;
    } catch (e) { $("pr-msg").className = "msg error"; $("pr-msg").textContent = e.message; }
  };
  $("pr-list").onclick = () => { $("pr-list").classList.add("active"); $("pr-eff").classList.remove("active"); renderList(); };
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 2 }); }
