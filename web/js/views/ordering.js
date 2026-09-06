/* Ordering — supplier sheet (forecast) → post order → ETL PO + confirmation CSV. */
"use strict";
export async function render(el, { API, SERVER }) {
  const suppliers = (await API.get("/api/ordering/suppliers")).suppliers;
  const branches = (await API.get("/api/payables/branches")).filter((b) => !SERVER.branch || b.id === SERVER.branch);

  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>Supplier</label>
      <select id="od-supplier">${suppliers.map((s) => `<option value="${s.code}">${s.code} · ${esc(s.name)}</option>`).join("")}</select>
      <label>Branch</label>
      <select id="od-branch"><option value="">All</option>${branches.map((b) => `<option value="${b.id}">${b.name}</option>`).join("")}</select>
      <button id="od-active" class="toggle-btn active">Active only</button>
      <button id="od-all" class="toggle-btn">Show all lines</button>
      <button id="od-load" class="secondary">Load sheet</button>
      <button id="od-settings" class="secondary" title="Ordering settings (modes + global switches)">⚙ Settings</button>
      <button id="od-repl" class="secondary" title="Inactive same-description predecessors (W6)">Replacement report</button>
      <div class="btn-group" id="od-post-group" style="display:none">
        <button id="od-post">Post order → ETL</button>
        <button id="od-csv" class="secondary" title="Download supplier confirmation CSV">CSV</button>
        <button id="od-export" class="secondary" title="Save sheet CSV to the server + download">Export CSV</button>
        <button id="od-print" class="secondary" title="Print the order sheet">🖨 Print</button>
        <button id="od-print-confirm" class="secondary" title="Print the posted PO confirmation" style="display:none">🖨 Confirmation</button>
      </div>
    </div>
    <div id="od-settings-panel" class="panel" style="display:none; margin:10px 0"></div>
    <div id="od-msg" class="msg"></div>
  </div>
  <div id="od-results" class="panel"><div class="placeholder"><h2>Load a supplier sheet</h2></div></div>`;

  const $ = (id) => el.querySelector(id.startsWith("#") ? id : "#" + id);
  let sheet = [];
  let showAll = false;
  const author = SERVER.author || SERVER.mode === "hos";

  // ── settings panel (modes + global switches) ─────────────────────────────
  $("od-settings").onclick = async () => {
    const panel = $("od-settings-panel");
    panel.style.display = panel.style.display === "none" ? "block" : "none";
    if (panel.style.display === "none") return;
    try {
      const s = await API.get("/api/ordering/settings");
      const m = (s.modes || []).find((x) => x.supplier_code === $("od-supplier").value);
      const mode = m || { mode: "weekly", lead_days: 3, cycle_days: 7, cover_days: null };
      panel.innerHTML = `
      <div class="toolbar" style="flex-wrap:wrap">
        <span class="stat-lbl">Supplier mode</span>
        <select id="os-mode" ${author ? "" : "disabled"}>
          <option value="weekly" ${mode.mode === "weekly" ? "selected" : ""}>Weekly</option>
          <option value="monthly" ${mode.mode === "monthly" ? "selected" : ""}>Monthly</option>
        </select>
        <label>Lead (days)</label><input id="os-lead" type="number" min="1" value="${mode.lead_days}" ${author ? "" : "disabled"} style="width:64px">
        <label>Cycle</label><input id="os-cycle" type="number" min="1" value="${mode.cycle_days}" ${author ? "" : "disabled"} style="width:64px">
        <label>Cover</label><input id="os-cover" type="number" min="0" value="${mode.cover_days ?? ""}" placeholder="auto" ${author ? "" : "disabled"} style="width:64px">
        <button id="os-save" class="secondary" ${author ? "" : "disabled"}>Save mode</button>
        <span style="margin:0 14px"></span>
        <label class="chk"><input type="checkbox" id="os-ignore-min" ${s.ignore_min_qty ? "checked" : ""} ${author ? "" : "disabled"}> Ignore min qty</label>
        <label class="chk"><input type="checkbox" id="os-ignore-max" ${s.ignore_max_qty ? "checked" : ""} ${author ? "" : "disabled"}> Ignore max qty</label>
        ${author ? `<button id="os-save-global" class="secondary">Save globals</button>` : `<span class="muted">(settings read-only on this install)</span>`}
      </div>`;
      $("#os-save").onclick = async () => {
        try {
          await API.send("POST", "/api/ordering/modes", {
            supplier_code: $("od-supplier").value,
            mode: $("#os-mode").value,
            lead_days: +$("#os-lead").value || 3,
            cycle_days: +$("#os-cycle").value || null,
            cover_days: $("#os-cover").value ? +$("#os-cover").value : null,
          });
          msg("Mode saved — reload the sheet to apply", "success");
        } catch (e) { msg(e.message, "error"); }
      };
      if (author) $("#os-save-global").onclick = async () => {
        try {
          await API.send("POST", "/api/ordering/settings", {
            ignore_min_qty: $("#os-ignore-min").checked,
            ignore_max_qty: $("#os-ignore-max").checked,
          });
          msg("Global switches saved (replicated to branches)", "success");
        } catch (e) { msg(e.message, "error"); }
      };
    } catch (e) { panel.innerHTML = `<p class="bad">${e.message}</p>`; }
  };

  // ── replacement report (W6 predecessor scan) ──────────────────────────────
  $("od-repl").onclick = async () => {
    msg("Loading replacement report…");
    try {
      const d = await API.get("/api/ordering/replacement-report");
      const rows = d.rows || [];
      const lvl = (v) => v === 3 ? "OLD_ sku" : v === 2 ? "same prod code" : v === 1 ? "same supplier" : "unmatched";
      $("od-results").innerHTML = rows.length
        ? `<div class="panel"><h3>Replacement report — inactive predecessors of active items <span class="muted">(${rows.length})</span></h3>
        <div class="table-wrap"><table>
        <thead><tr><th>Active item</th><th>Inactive predecessor</th><th>Old SKU</th><th>Match</th><th>Suggested OLD_ SKU</th></tr></thead>
        <tbody>${rows.map((r) => `<tr>
          <td>${esc(r.description)}<div class="muted">${esc(r.new_upc)} · sup ${esc(r.new_supplier)}</div></td>
          <td class="muted">${esc(r.old_upc)} <span class="muted">· sup ${esc(r.old_supplier)}</span></td>
          <td class="muted">${esc(r.old_sku)}</td>
          <td>${lvl(r.match_level)}</td>
          <td><code>${esc(r.suggested_sku)}</code></td></tr>`).join("")}</tbody></table></div></div>`
        : '<div class="placeholder"><h2>No replacements found — every active item has distinct history</h2></div>';
      msg(`Replacement report: ${rows.length} candidates`);
    } catch (e) { msg(e.message, "error"); }
  };

  function msg(text, cls) {
    $("od-msg").className = cls ? `msg ${cls}` : "msg";
    $("od-msg").textContent = text;
  }

  $("od-load").onclick = async () => {
    const sup = $("od-supplier").value, br = $("od-branch").value;
    const active = $("od-active").classList.contains("active") ? "&active_only=true" : "";
    msg("Loading…");
    try {
      const d = await API.get(`/api/ordering/sheet?supplier=${sup}${br ? `&branch=${br}` : ""}${active}`);
      sheet = d.lines || [];
      renderSheet();
      msg(`${sheet.length} lines · supplier ${d.supplier}`);
    } catch (e) { msg(e.message, "error"); }
  };
  $("od-active").onclick = () => $("od-active").classList.toggle("active");
  $("od-all").onclick = () => {
    showAll = !showAll;
    $("od-all").classList.toggle("active", showAll);
    renderSheet();
  };

  function renderSheet() {
    const shown = showAll ? sheet : sheet.filter((l) => l.result.suggested > 0 || l.on_order > 0);
    const body = shown.map((l, i) => `
      <tr>
        <td>${esc(l.description)}<div class="muted">${l.upc}</div></td>
        <td class="num">${l.result.rate30 ? l.result.rate30.toFixed(2) : "—"}</td>
        <td class="num">${l.result.rate90 ? l.result.rate90.toFixed(2) : "—"}</td>
        <td class="num">${l.on_hand}</td>
        <td class="num">${l.on_order}</td>
        <td class="num">${l.result.sellout_days}</td>
        <td class="num">${l.result.suggested}</td>
        <td class="num">${fmt$(l.unit_cost)}</td>
        <td class="num"><span class="od-num"><input type="number" min="0" step="1" value="${l.result.suggested}" data-upc="${l.upc}" class="od-qty" style="width:64px">
          <span class="od-num-arrows"><button type="button" class="od-qty-up" data-upc="${l.upc}" tabindex="-1" aria-label="increment">▲</button><button type="button" class="od-qty-down" data-upc="${l.upc}" tabindex="-1" aria-label="decrement">▼</button></span></span></td>
        <td class="num" data-total="${l.upc}">${fmt$(l.result.suggested * l.unit_cost)}</td>
      </tr>`).join("");
    $("od-post-group").style.display = "inline-flex";
    $("od-results").innerHTML = `<div class="table-wrap"><table>
      <colgroup><col class="c-desc"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
      <thead><tr><th>Description</th><th class="num">Fwd/Day</th><th class="num">Sold 90d</th><th class="num">SOH</th><th class="num">OnOrder</th><th class="num">Sellout</th><th class="num">Suggested</th><th class="num">UnitCost</th><th class="num">Qty</th><th class="num">$</th></tr></thead>
      <tbody>${body}</tbody></table></div>`;
    for (const inp of el.querySelectorAll(".od-qty")) {
      inp.oninput = () => {
        el.querySelector(`[data-total="${inp.dataset.upc}"]`).textContent = fmt$(+inp.value * lineFor(inp.dataset.upc).unit_cost);
      };
      // keyboard nav: Enter/Tab move to next qty box, Shift+Tab previous;
      // selecting a box selects its value so the operator can type over it.
      inp.addEventListener("keydown", (e) => {
        if (e.key === "Tab" || e.key === "Enter") {
          e.preventDefault();
          const all = Array.from(el.querySelectorAll(".od-qty"));
          const idx = all.indexOf(inp);
          const dir = (e.key === "Tab" && e.shiftKey) ? -1 : 1;
          const next = all[idx + dir];
          if (next) next.focus();
          else if (e.key === "Enter") inp.blur();
        } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          const cur = parseFloat(inp.value) || 0;
          inp.value = String(Math.max(0, cur + (e.key === "ArrowUp" ? 1 : -1)));
          inp.dispatchEvent(new Event("input", { bubbles: true }));
        }
      });
      inp.addEventListener("focus", () => inp.select());
    }
    // custom steppers: step the qty and re-fire input
    el.querySelectorAll(".od-num").forEach((wrap) => {
      const inp = wrap.querySelector("input");
      const up = wrap.querySelector(".od-qty-up");
      const down = wrap.querySelector(".od-qty-down");
      const step = (dir) => {
        if (!inp) return;
        const cur = parseFloat(inp.value) || 0;
        inp.value = String(Math.max(0, cur + dir));
        inp.dispatchEvent(new Event("input", { bubbles: true }));
      };
      if (up) up.onclick = () => step(1);
      if (down) down.onclick = () => step(-1);
    });
  }
  function lineFor(upc) { return sheet.find((l) => l.upc === upc) || { unit_cost: 0 }; }

  function qtyLines() {
    const lines = [];
    for (const inp of el.querySelectorAll(".od-qty")) {
      const qty = +inp.value;
      if (qty > 0) {
        const l = lineFor(inp.dataset.upc);
        lines.push({ upc: l.upc, qty, unit_cost: l.unit_cost, suggested_qty: l.result ? l.result.suggested : 0 });
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
      window._lastOrder = { supplier: $("od-supplier").value, branch: $("od-branch").value, lines, po_file: r.po_file, status: r.status, bol: r.bill_of_lading };
      $("od-print-confirm").style.display = "";
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

  $("od-print").onclick = () => printSheet(sheet);
  $("od-print-confirm").onclick = () => printConfirmation(window._lastOrder);

  $("od-export").onclick = async () => {
    const lines = qtyLines();
    if (!lines.length) { $("od-msg").className = "msg warn"; $("od-msg").textContent = "No quantities entered."; return; }
    try {
      const r = await API.send("POST", "/api/ordering/export", {
        supplier: $("od-supplier").value,
        branch: $("od-branch").value ? +$("od-branch").value : undefined,
        lines,
      });
      download(r.filename, r.content, "text/csv");
      $("od-msg").className = "msg success";
      $("od-msg").textContent = `Saved ${r.filename} (server) + downloaded`;
    } catch (e) { $("od-msg").className = "msg error"; $("od-msg").textContent = e.message; }
  };
}

function printConfirmation(o) {
  if (!o || !o.lines || !o.lines.length) return;
  const asOf = new Date().toLocaleString();
  const total = o.lines.reduce((a, l) => a + l.qty * l.unit_cost, 0);
  const w = window.open("", "_blank", "width=900,height=700");
  if (!w) return;
  w.document.write(`<html><head><title>PO Confirmation</title><style>
    body{font-family:system-ui,sans-serif;margin:24px;color:#222}
    h1{font-size:18px;margin:0 0 4px}.sub{color:#666;font-size:12px;margin-bottom:14px}
    .kv{display:grid;grid-template-columns:auto 1fr;gap:2px 14px;font-size:12px;margin-bottom:14px}
    .kv b{color:#666;font-weight:600}
    table{width:100%;border-collapse:collapse;font-size:12px}
    th,td{border:1px solid #ccc;padding:4px 6px;text-align:right}
    th{background:#f0f0f0}.l{text-align:left}.total td{border-top:2px solid #333;font-weight:bold}
    @media print{body{margin:8mm}}
  </style></head><body>
    <h1>Supplier Order Confirmation</h1>
    <div class="sub">${asOf} · ${o.status || ""} · Bol ${o.bol || ""} · ${o.po_file || ""}</div>
    <div class="kv"><b>Supplier</b><span>${esc(o.supplier)}</span><b>Branch</b><span>${esc(o.branch || "All")}</span></div>
    <table><thead><tr><th class="l">UPC</th><th class="l">Description</th><th class="num">Qty</th><th class="num">Unit</th><th class="num">Total</th></tr></thead>
    <tbody>${o.lines.map((l) => `<tr><td class="l">${esc(l.upc)}</td><td class="l">${esc(l.description || "")}</td><td>${l.qty}</td><td>${l.unit_cost.toFixed(2)}</td><td>${(l.qty * l.unit_cost).toFixed(2)}</td></tr>`).join("")}
    <tr class="total"><td colspan="4" class="l">Total</td><td>${total.toFixed(2)}</td></tr></tbody></table>
    <script>window.onload=function(){setTimeout(function(){window.print();},300)}<\/script>
  </body></html>`);
  w.document.close();
}
function printSheet(sheet) {
  if (!sheet || !sheet.length) return;
  const qtyOf = (upc) => document.querySelector(`.od-qty[data-upc="${upc}"]`)?.value ?? "";
  const lines = sheet.filter((l) => +qtyOf(l.upc) > 0 || (l.result && l.result.suggested > 0)).slice(0, 200);
  const asOf = new Date().toLocaleString();
  const rows = lines.map((l) => `<tr>
      <td class="l">${esc(l.description)}<div style="color:#888;font-size:10px">${esc(l.upc)}</div></td>
      <td>${l.result && l.result.rate30 ? l.result.rate30.toFixed(2) : "—"}</td>
      <td>${l.on_hand}</td><td>${l.on_order}</td>
      <td>${l.result ? l.result.sellout_days : ""}</td>
      <td>${l.result ? l.result.suggested : ""}</td>
      <td>${esc(qtyOf(l.upc)) || (l.result ? l.result.suggested : 0)}</td>
    </tr>`).join("");
  const w = window.open("", "_blank", "width=900,height=700");
  if (!w) return;
  w.document.write(`<html><head><title>Order Sheet</title><style>
    body{font-family:system-ui,sans-serif;margin:24px;color:#222}
    h1{font-size:18px;margin:0 0 4px}.sub{color:#666;font-size:12px;margin-bottom:14px}
    table{width:100%;border-collapse:collapse;font-size:12px}
    th,td{border:1px solid #ccc;padding:4px 6px;text-align:right}
    th{background:#f0f0f0}.l{text-align:left}
    @media print{body{margin:8mm}}
  </style></head><body>
    <h1>Supplier Order Sheet</h1>
    <div class="sub">${asOf} · ${lines.length} lines</div>
    <table><thead><tr><th class="l">Description</th><th>Fwd/Day</th><th>SOH</th><th>OnOrder</th><th>Sellout</th><th>Suggested</th><th class="l">Qty</th></tr></thead>
    <tbody>${rows}</tbody></table>
    <script>window.onload=function(){setTimeout(function(){window.print();},300)}<\/script>
  </body></html>`);
  w.document.close();
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
