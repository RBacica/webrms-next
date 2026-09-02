/* Ordering — supplier sheet (forecast) → post order → ETL PO + confirmation CSV. */
"use strict";
export async function render(el, { API, SERVER }) {
  const suppliers = (await API.get("/api/ordering/suppliers")).suppliers;
  const branches = (await API.get("/api/payables/branches")).filter((b) => !SERVER.branch || b.id === SERVER.branch);

  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>Supplier</label>
      <select id="od-supplier">${suppliers.map((s) => `<option value="${s.code}">${s.label}</option>`).join("")}</select>
      <label>Branch</label>
      <select id="od-branch"><option value="">All</option>${branches.map((b) => `<option value="${b.id}">${b.name}</option>`).join("")}</select>
      <button id="od-active" class="toggle-btn active">Active only</button>
      <button id="od-load" class="secondary">Load sheet</button>
      <div class="btn-group" id="od-post-group" style="display:none">
        <button id="od-post">Post order → ETL</button>
        <button id="od-csv" class="secondary" title="Download supplier confirmation CSV">CSV</button>
      </div>
    </div>
    <div id="od-msg" class="msg"></div>
  </div>
  <div id="od-results" class="panel"><div class="placeholder"><h2>Load a supplier sheet</h2></div></div>`;

  const $ = (id) => el.querySelector(id);
  let sheet = [];

  $("od-load").onclick = async () => {
    const sup = $("od-supplier").value, br = $("od-branch").value;
    const active = $("od-active").classList.contains("active") ? "&active_only=true" : "";
    $("od-msg").className = "msg";
    $("od-msg").textContent = "Loading…";
    try {
      const d = await API.get(`/api/ordering/sheet?supplier=${sup}${br ? `&branch=${br}` : ""}${active}`);
      sheet = d.lines || [];
      renderSheet();
      $("od-msg").textContent = `${sheet.length} lines · supplier ${d.supplier}`;
    } catch (e) {
      $("od-msg").className = "msg error";
      $("od-msg").textContent = e.message;
    }
  };
  $("od-active").onclick = () => $("od-active").classList.toggle("active");

  function renderSheet() {
    const withQty = sheet.filter((l) => l.result.suggested > 0 || l.on_order > 0);
    const body = withQty.map((l, i) => `
      <tr>
        <td>${esc(l.description)}<div class="muted">${l.upc}</div></td>
        <td class="num">${l.result.rate30 ? l.result.rate30.toFixed(2) : "—"}</td>
        <td class="num">${l.result.rate90 ? l.result.rate90.toFixed(2) : "—"}</td>
        <td class="num">${l.on_hand}</td>
        <td class="num">${l.on_order}</td>
        <td class="num">${l.result.sellout_days}</td>
        <td class="num">${l.result.suggested}</td>
        <td class="num">${fmt$(l.unit_cost)}</td>
        <td class="num"><input type="number" min="0" step="1" value="${l.result.suggested}" data-i="${i}" class="od-qty" style="width:76px"></td>
        <td class="num" data-total="${i}">${fmt$(l.result.suggested * l.unit_cost)}</td>
      </tr>`).join("");
    $("od-post-group").style.display = "inline-flex";
    $("od-results").innerHTML = `<div class="table-wrap"><table>
      <colgroup><col class="c-desc"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
      <thead><tr><th>Description</th><th class="num">Fwd/Day</th><th class="num">Sold 90d</th><th class="num">SOH</th><th class="num">OnOrder</th><th class="num">Sellout</th><th class="num">Suggested</th><th class="num">UnitCost</th><th class="num">Qty</th><th class="num">$</th></tr></thead>
      <tbody>${body}</tbody></table></div>`;
    for (const inp of el.querySelectorAll(".od-qty")) {
      inp.oninput = () => {
        const i = +inp.dataset.i;
        el.querySelector(`[data-total="${i}"]`).textContent = fmt$(+inp.value * sheet[i].unit_cost);
      };
    }
  }

  function qtyLines() {
    const lines = [];
    for (const inp of el.querySelectorAll(".od-qty")) {
      const qty = +inp.value;
      if (qty > 0) {
        const l = sheet[+inp.dataset.i];
        lines.push({ upc: l.upc, qty, unit_cost: l.unit_cost, suggested_qty: l.result.suggested });
      }
    }
    return lines;
  }

  $("od-post").onclick = async () => {
    const lines = qtyLines();
    if (!lines.length) { $("od-msg").className = "msg warn"; $("od-msg").textContent = "No quantities entered."; return; }
    try {
      const r = await API.send("POST", "/api/ordering/orders", {
        supplier: $("od-supplier").value,
        branch: $("od-branch").value ? +$("od-branch").value : undefined,
        by: "ui",
        lines,
      });
      $("od-msg").className = "msg success";
      $("od-msg").textContent = `PO ${r.po_file} → ${r.status} (Bol ${r.bill_of_lading})`;
      window._lastOrderId = r.order_id;
    } catch (e) { $("od-msg").className = "msg error"; $("od-msg").textContent = e.message; }
  };

  $("od-csv").onclick = async () => {
    if (!window._lastOrderId) { $("od-msg").className = "msg warn"; $("od-msg").textContent = "Post an order first."; return; }
    try {
      const r = await API.get(`/api/ordering/confirmation-csv?order_id=${window._lastOrderId}`);
      download(r.filename, r.content, "text/csv");
      $("od-msg").className = "msg success";
      $("od-msg").textContent = `Downloaded ${r.filename}`;
    } catch (e) { $("od-msg").className = "msg error"; $("od-msg").textContent = e.message; }
  };
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 2 }); }
function download(name, content, type) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([content], { type }));
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
