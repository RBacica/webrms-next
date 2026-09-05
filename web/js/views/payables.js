/* Payables — bills net of paid_ledger, returns, terms config (HoS write). */
"use strict";
export async function render(el, { API, SERVER }) {
  const branches = (await API.get("/api/payables/branches")).filter((b) => !SERVER.branch || b.id === SERVER.branch);
  const suppliers = (await API.get("/api/payables/suppliers"));
  const today = new Date();
  const iso = (d) => d.toISOString().slice(0, 10);
  const from = iso(new Date(today.getTime() - 60 * 864e5));
  const to = iso(new Date(today.getTime() + 864e5));

  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>From</label><input type="date" id="pa-from" value="${from}">
      <label>To</label><input type="date" id="pa-to" value="${to}">
      <label>Branch</label>
      <select id="pa-branch"><option value="">All</option>${branches.map((b) => `<option value="${b.id}">${b.name}</option>`).join("")}</select>
      <label>Supplier</label>
      <select id="pa-supplier"><option value="">All</option>${suppliers.map((s) => `<option value="${s.code}">${s.label}</option>`).join("")}</select>
      <div class="btn-group">
        <button id="pa-load" class="active">Bills</button>
        <button id="pa-returns" class="secondary">Returns</button>
      </div>
      <button id="pa-pay" class="secondary" style="margin-left:auto">Mark selected paid</button>
    </div>
    <div id="pa-msg" class="msg"></div>
  </div>
  <div id="pa-results" class="panel"><div class="placeholder"><h2>Load bills</h2></div></div>`;

  const $ = (id) => el.querySelector(id.startsWith("#") ? id : "#" + id);
  let mode = "bills";
  let bills = [];

  async function load() {
    const q = `from=${$("pa-from").value}&to=${$("pa-to").value}&branch=${$("pa-branch").value || ""}&supplier=${$("pa-supplier").value || ""}`;
    try {
      const d = await API.get(`/api/payables/${mode === "bills" ? "invoices" : "returns"}?${q}`);
      bills = d;
      renderRows();
    } catch (e) { $("pa-msg").className = "msg error"; $("pa-msg").textContent = e.message; }
  }
  $("pa-load").onclick = () => { mode = "bills"; $("pa-load").classList.add("active"); $("pa-returns").classList.remove("active"); load(); };
  $("pa-returns").onclick = () => { mode = "returns"; $("pa-returns").classList.add("active"); $("pa-load").classList.remove("active"); load(); };

  function renderRows() {
    if (mode === "bills") {
      const total = bills.reduce((a, b) => a + b.invoice_amount, 0);
      $("pa-results").innerHTML = `<div class="stats-row"><div class="stat"><span class="stat-val">${bills.length}</span><span class="stat-lbl">Bills</span></div>
        <div class="stat"><span class="stat-val">${fmt$(total)}</span><span class="stat-lbl">Total due</span></div></div>
        <div class="table-wrap"><table><colgroup><col class="c-sel"><col><col><col class="c-num"><col class="c-num"><col><col class="c-num"></colgroup>
        <thead><tr><th></th><th>Supplier</th><th>Invoice</th><th class="num">Amount</th><th class="num">Tax</th><th>Due</th><th>Terms</th></tr></thead>
        <tbody>${bills.map((b, i) => `<tr>
          <td><input type="checkbox" data-i="${i}" class="pa-sel"></td>
          <td>${esc(b.supplier_code)}</td><td>${esc(b.invoice_number)}</td>
          <td class="num">${fmt$(b.invoice_amount)}</td><td class="num">${fmt$(b.tax_amount)}</td>
          <td>${esc(b.due_date)}</td><td>${b.terms_unset ? '<span class="warn">EOM+20 (unset)</span>' : "configured"}</td>
        </tr>`).join("")}</tbody></table></div>`;
    } else {
      $("pa-results").innerHTML = `<div class="table-wrap"><table><colgroup><col><col><col class="c-num"><col></colgroup>
        <thead><tr><th>Supplier</th><th>Ref</th><th class="num">Credit</th><th>Date</th></tr></thead>
        <tbody>${bills.map((b) => `<tr><td>${esc(b.supplier_code)}</td><td>${esc(b.invoice_number)}</td><td class="num">${fmt$(b.invoice_amount)}</td><td>${esc(b.invoice_date)}</td></tr>`).join("")}</tbody></table></div>`;
    }
  }

  $("pa-pay").onclick = async () => {
    if (mode !== "bills") return;
    const rows = [];
    for (const c of el.querySelectorAll(".pa-sel:checked")) {
      const b = bills[+c.dataset.i];
      rows.push({ branch_id: b.branch, supplier_code: b.supplier_code, invoice_number: b.invoice_number, amount: b.invoice_amount });
    }
    if (!rows.length) { $("pa-msg").className = "msg warn"; $("pa-msg").textContent = "Select invoices to mark paid."; return; }
    try {
      const r = await API.send("POST", "/api/payables/pay", { rows });
      $("pa-msg").className = "msg success";
      $("pa-msg").textContent = r.message;
      load();
    } catch (e) { $("pa-msg").className = "msg error"; $("pa-msg").textContent = e.message; }
  };
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 2 }); }
