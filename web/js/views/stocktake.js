/* Stocktake — search items w/ SOH → count → variance highlight (barcode UX). */
"use strict";
export async function render(el, { API, SERVER }) {
  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>Branch</label>
      <select id="st-branch">${SERVER.branch ? `<option value="${SERVER.branch}">${SERVER.branch}</option>` : ""}</select>
      <input type="search" id="st-q" placeholder="Scan barcode or type to search…" autofocus>
      <button id="st-search" class="secondary">Search</button>
      <div class="btn-group" style="margin-left:auto">
        <button id="st-save">Save counts</button>
        <button id="st-clear" class="secondary">Reset</button>
      </div>
    </div>
    <div id="st-msg" class="msg"></div>
  </div>
  <div id="st-results" class="panel"><div class="placeholder"><h2>Scan a barcode</h2></div></div>`;

  const $ = (id) => el.querySelector(id);
  let rows = [];
  const branch = SERVER.branch || "";

  async function search(q) {
    if (!q) return;
    $("st-msg").textContent = "…";
    try {
      const d = await API.get(`/api/stocktake/search?branch=${branch}${q ? `&q=${encodeURIComponent(q)}` : ""}`);
      rows = d.items || [];
      $("st-msg").textContent = `${rows.length} items`;
      renderRows();
    } catch (e) { $("st-msg").className = "msg error"; $("st-msg").textContent = e.message; }
  }
  $("st-search").onclick = () => search($("st-q").value);
  $("st-q").onkeydown = (e) => { if (e.key === "Enter") search($("st-q").value); };
  $("st-clear").onclick = () => { rows = []; $("st-results").innerHTML = '<div class="placeholder"><h2>Scan a barcode</h2></div>'; $("st-q").value = ""; $("st-q").focus(); };

  function renderRows() {
    $("st-results").innerHTML = `<div class="table-wrap"><table>
      <colgroup><col><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
      <thead><tr><th>Description</th><th class="num">SOH</th><th class="num">Count</th><th class="num">Variance</th></tr></thead>
      <tbody>${rows.map((r, i) => {
        const v = r.count !== null && r.count !== undefined ? r.count - r.stock_on_hand : null;
        return `<tr class="${v !== null && v !== 0 ? "has-variance" : ""}">
          <td>${esc(r.description)}<div class="muted">${r.upc}</div></td>
          <td class="num">${r.stock_on_hand}</td>
          <td class="num"><input type="number" min="0" step="1" data-i="${i}" class="st-count" style="width:76px" value="${r.count ?? ""}"></td>
          <td class="num">${v === null ? "—" : v}</td>
        </tr>`; }).join("")}</tbody></table></div>`;
    for (const inp of el.querySelectorAll(".st-count")) {
      inp.oninput = () => updateVariance(+inp.dataset.i);
    }
  }
  function updateVariance(i) {
    const inp = el.querySelector(`.st-count[data-i="${i}"]`);
    const count = inp.value === "" ? null : +inp.value;
    const v = count === null ? null : count - rows[i].stock_on_hand;
    const td = inp.closest("tr").querySelector("td:last-child");
    td.textContent = v === null ? "—" : v;
    inp.closest("tr").classList.toggle("has-variance", v !== null && v !== 0);
  }
  $("st-save").onclick = () => {
    const saved = [];
    for (const inp of el.querySelectorAll(".st-count")) {
      const i = +inp.dataset.i;
      if (inp.value !== "") saved.push({ upc: rows[i].upc, count: +inp.value, stock_on_hand: rows[i].stock_on_hand });
    }
    $("st-msg").className = "msg success";
    $("st-msg").textContent = `${saved.length} counts captured (export UI lands in a later phase).`;
  };
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
