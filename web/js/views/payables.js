/* Payables — bills net of paid_ledger, returns, supplier terms config (HoS
   write), export TSV, paid-ledger awareness. Mirrors the old WebRMS payables
   UI (per-supplier terms editor, export, print-due) over the local DB. */
"use strict";
export async function render(el, { API, SERVER }) {
  const author = SERVER.author || SERVER.mode === "hos";
  const branches = (await API.get("/api/payables/branches")).filter((b) => !SERVER.branch || b.id === SERVER.branch);
  const suppliers = (await API.get("/api/payables/suppliers"));
  const today = new Date();
  const iso = (d) => d.toISOString().slice(0, 10);
  const from = iso(new Date(today.getTime() - 60 * 864e5));
  const to = iso(new Date(today.getTime() + 864e5));
  let paidSet = new Set();
  try { paidSet = new Set((await API.get("/api/payables/paid")).paid || []); } catch { /* non-fatal */ }

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
      <button id="pa-export" class="secondary">Export TSV</button>
      ${author ? `<button id="pa-terms" class="secondary">Supplier terms</button>` : ""}
    </div>
    <div id="pa-msg" class="msg"></div>
  </div>
  <div id="pa-results" class="panel"><div class="placeholder"><h2>Load bills</h2></div></div>
  <div id="pa-terms-panel" class="panel" style="display:none"></div>`;

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
      const paidKey = (b) => `${b.branch_id ?? b.branch ?? 0}|${b.supplier_code}|${b.invoice_number}`;
      const selCol = author ? `<col class="c-sel">` : "";
      const selHead = author ? "<th></th>" : "";
      $("pa-results").innerHTML = `<div class="stats-row"><div class="stat"><span class="stat-val">${bills.length}</span><span class="stat-lbl">Bills</span></div>
        <div class="stat"><span class="stat-val">${fmt$(total)}</span><span class="stat-lbl">Total due</span></div></div>
        <div class="table-wrap"><table><colgroup>${selCol}<col><col><col class="c-num"><col class="c-num"><col><col class="c-num"><col></colgroup>
        <thead><tr>${selHead}<th>Supplier</th><th>Invoice</th><th class="num">Amount</th><th class="num">Tax</th><th>Due</th><th>Terms</th><th>Status</th></tr></thead>
        <tbody>${bills.map((b, i) => {
          const isPaid = paidSet.has(paidKey(b));
          return `<tr class="${isPaid ? "row-paid" : ""}">
            ${author ? `<td><input type="checkbox" data-i="${i}" class="pa-sel" ${isPaid ? "disabled" : ""}></td>` : ""}
            <td>${esc(b.supplier_code)}</td><td>${esc(b.invoice_number)}</td>
            <td class="num">${fmt$(b.invoice_amount)}</td><td class="num">${fmt$(b.tax_amount)}</td>
            <td>${esc(b.due_date)}</td><td>${b.terms_unset ? '<span class="warn">EOM+20 (unset)</span>' : "configured"}</td>
            <td>${isPaid ? '<span class="ok">paid</span>' : '<span class="muted">open</span>'}</td>
          </tr>`;
        }).join("")}</tbody></table></div>`;
    } else {
      $("pa-results").innerHTML = `<div class="table-wrap"><table><colgroup><col><col><col class="c-num"><col></colgroup>
        <thead><tr><th>Supplier</th><th>Ref</th><th class="num">Credit</th><th>Date</th></tr></thead>
        <tbody>${bills.map((b) => `<tr><td>${esc(b.supplier_code)}</td><td>${esc(b.invoice_number)}</td><td class="num">${fmt$(b.invoice_amount)}</td><td>${esc(b.invoice_date)}</td></tr>`).join("")}</tbody></table></div>`;
    }
  }

  $("pa-pay").onclick = async () => {
    if (mode !== "bills" || !author) return;
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
      rows.forEach((x) => paidSet.add(`${x.branch_id}|${x.supplier_code}|${x.invoice_number}`));
      load();
    } catch (e) { $("pa-msg").className = "msg error"; $("pa-msg").textContent = e.message; }
  };

  $("pa-export").onclick = async () => {
    if (mode !== "bills" || !bills.length) return;
    const rows = bills.map((b) => ({
      branch: String(b.branch_id ?? b.branch ?? ""), supplier_code: b.supplier_code, invoice_number: b.invoice_number,
      description: b.description || "", invoice_date: b.invoice_date, invoice_amount: b.invoice_amount,
      po_number: b.po_number || "", tax_amount: b.tax_amount, due_date: b.due_date,
    }));
    try {
      const r = await API.send("POST", "/api/payables/export", { rows });
      download("payables-export.tsv", r.tsv, "text/tab-separated-values");
      $("pa-msg").className = "msg success";
      $("pa-msg").textContent = `Exported ${rows.length} rows (also saved to data/output/payables-export.tsv)`;
    } catch (e) { $("pa-msg").className = "msg error"; $("pa-msg").textContent = e.message; }
  };

  if (author) $("pa-terms").onclick = async () => {
    const panel = $("pa-terms-panel");
    const showing = panel.style.display !== "none";
    panel.style.display = showing ? "none" : "";
    if (showing) return;
    panel.innerHTML = '<div class="placeholder"><h2>Loading supplier terms…</h2></div>';
    try {
      const cfg = await API.get("/api/payables/config");
      const entries = cfg.suppliers || cfg;
      panel.innerHTML = `<div class="toolbar">
        <label>Filter</label><input type="text" id="sterm-filter" placeholder="code or name…">
        <span style="flex:1"></span>
        <button id="sterm-bulk" class="secondary">Bulk Save All</button>
      </div>
      <div class="table-wrap"><table><colgroup><col><col><col><col class="c-num"><col><col><col></colgroup>
        <thead><tr><th>Code</th><th>Label</th><th>Order type</th><th>Term</th><th class="num">Days</th><th>Payment</th><th></th></tr></thead>
        <tbody id="sterm-body"></tbody></table></div>
      <p class="muted" style="font-size:12px">Unconfigured suppliers use EOM + 20 days by default.</p>
      <div id="sterm-msg" class="msg"></div>`;
      const body = el.querySelector("#sterm-body");
      const render = (q) => {
        body.innerHTML = entries.filter((e) => !q || e.code.toLowerCase().includes(q) || (e.label || "").toLowerCase().includes(q))
          .map((e) => `<tr>
            <td class="muted">${esc(e.code)}</td><td>${esc(e.label || "")}</td>
            <td class="muted">${esc(e.order_type || "—")}</td>
            <td><select data-code="${esc(e.code)}" class="sterm-type">
              <option value="EOM" ${e.term_type === "EOM" ? "selected" : ""}>EOM</option>
              <option value="NetDays" ${e.term_type === "NetDays" ? "selected" : ""}>NetDays</option>
            </select></td>
            <td><input type="number" class="sterm-days" data-code="${esc(e.code)}" value="${e.term_days ?? 20}" min="0" max="999" style="width:70px"></td>
            <td><select data-code="${esc(e.code)}" class="sterm-order">
              <option value="Monthly" ${e.order_type === "Monthly" ? "selected" : ""}>Monthly</option>
              <option value="Weekly" ${e.order_type === "Weekly" ? "selected" : ""}>Weekly</option>
            </select></td>
            <td><button class="secondary sterm-save" data-code="${esc(e.code)}">✓ Save</button></td>
          </tr>`).join("");
      };
      render("");
      const filter = el.querySelector("#sterm-filter");
      filter.oninput = (ev) => render(ev.target.value.trim().toLowerCase());
      const msg = el.querySelector("#sterm-msg");
      for (const b of el.querySelectorAll(".sterm-save")) {
        b.onclick = async () => {
          const code = b.dataset.code;
          const type = el.querySelector(`.sterm-type[data-code="${code}"]`).value;
          const days = +el.querySelector(`.sterm-days[data-code="${code}"]`).value || 0;
          const order = el.querySelector(`.sterm-order[data-code="${code}"]`).value;
          try {
            await API.send("POST", "/api/payables/config", { supplier_code: code, term_type: type, term_days: days, order_type: order, payment_type: "DC" });
            msg.className = "msg success"; msg.textContent = `Saved ${code}`;
          } catch (e) { msg.className = "msg error"; msg.textContent = e.message; }
        };
      }
      const bulk = el.querySelector("#sterm-bulk");
      bulk.onclick = async () => {
        const rows = [];
        for (const b of el.querySelectorAll(".sterm-save")) {
          const code = b.dataset.code;
          rows.push({
            supplier_code: code,
            term_type: el.querySelector(`.sterm-type[data-code="${code}"]`).value,
            term_days: +el.querySelector(`.sterm-days[data-code="${code}"]`).value || 0,
            order_type: el.querySelector(`.sterm-order[data-code="${code}"]`).value,
          });
        }
        try {
          await API.send("POST", "/api/payables/config-bulk", { suppliers: rows });
          msg.className = "msg success"; msg.textContent = `Saved ${rows.length} suppliers`;
        } catch (e) { msg.className = "msg error"; msg.textContent = e.message; }
      };
    } catch (e) {
      panel.innerHTML = `<div class="msg error">${esc(e.message)}</div>`;
    }
  };

  for (const id of ["pa-from", "pa-to", "pa-branch", "pa-supplier"]) $(id).onchange = load;
  load();
}

function esc(s) { const d = document.createElement("div"); d.textContent = s == null ? "" : s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 2 }); }
function download(name, content, type) {
  const blob = content instanceof Blob ? content : new Blob([content], { type });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
