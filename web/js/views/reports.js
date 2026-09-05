/* Reports — old-WebRMS Reports view ported over the local DB.
   Pages: Daily Sales · Dept & Product · Stock Valuation · Stocktake & Shrink ·
   GRN ↔ AP · Promo Effectiveness. All read-only, date/branch aware. */
"use strict";

const REPORTS = [
  { id: "daily", label: "Daily Sales" },
  { id: "depts", label: "Dept & Product" },
  { id: "stock", label: "Stock Valuation" },
  { id: "stocktakes", label: "Stocktake & Shrink" },
  { id: "receipts", label: "GRN ↔ AP" },
  { id: "promo", label: "Promo Effectiveness" },
];

export async function render(el, { API, SERVER }) {
  const now = new Date();
  const to = iso(now);
  const from = iso(new Date(now.getFullYear(), now.getMonth() - 1, 1));
  el.innerHTML = `
  <div class="panel">
    <div class="toolbar">
      <label>From</label><input type="date" id="rp-from" value="${from}">
      <label>To</label><input type="date" id="rp-to" value="${to}">
      ${SERVER.author || SERVER.mode === "hos" ? `<select id="rp-branch">${branchOpts(SERVER)}</select>` : ""}
      <div class="btn-group" id="rp-tabs">
        ${REPORTS.map((r, i) => `<button data-rp="${r.id}" class="${i === 0 ? "active" : "secondary"}">${r.label}</button>`).join("")}
      </div>
      <button id="rp-csv" class="secondary">CSV</button>
    </div>
    <div id="rp-msg" class="msg"></div>
  </div>
  <div id="rp-results" class="panel"><div class="placeholder"><h2>Choose a report</h2></div></div>`;

  const $ = (id) => el.querySelector(id);
  let current = "daily";
  let lastData = null;

  const branchQ = () => {
    const b = $("#rp-branch")?.value;
    return b && b !== "ALL" ? `&branch=${b}` : "";
  };

  async function run() {
    const f = $("#rp-from").value, t = $("#rp-to").value;
    $("#rp-msg").textContent = "";
    const res = $("#rp-results");
    res.innerHTML = '<div class="placeholder"><h2>Loading…</h2></div>';
    try {
      switch (current) {
        case "daily": {
          const d = await API.get(`/api/reports/daily?from=${f}&to=${t}${branchQ()}`);
          lastData = { head: ["Date", "Txns", "Gross", "Cost", "Net"], rows: d.daily.map((r) => [r.date, r.txns, `$${r.gross_total}`, `$${r.cost}`, `$${r.net}`]) };
          res.innerHTML = tbl(["Date", "Txns", "Gross (excl)", "Cost", "Net"],
            d.daily.map((r) => `<tr><td>${esc(r.date)}</td><td class="num">${r.txns}</td><td class="num">${fmt$(r.gross_total)}</td><td class="num">${fmt$(r.cost)}</td><td class="num">${fmt$(r.net)}</td></tr>`).join("") +
            `<tr class="ov-total-row"><td>${d.totals.date}</td><td class="num">${d.totals.txns}</td><td class="num">${fmt$(d.totals.gross_total)}</td><td class="num">${fmt$(d.totals.cost)}</td><td class="num">${fmt$(d.totals.net)}</td></tr>`);
          break;
        }
        case "depts": {
          const d = await API.get(`/api/reports/depts?from=${f}&to=${t}${branchQ()}`);
          lastData = null;
          res.innerHTML = d.map((dept) => `
            <div class="panel" style="margin-bottom:10px">
              <h3>${esc(dept.dept_name)} <span class="muted">· target ${dept.target_margin}% · ${fmt$(dept.net)} net / ${fmt$(dept.cost)} cost</span></h3>
              <div class="table-wrap"><table>
                <thead><tr><th>Product</th><th class="num">Units</th><th class="num">Net</th><th class="num">Margin</th></tr></thead>
                <tbody>${dept.products.slice(0, 50).map((p) => `<tr>
                  <td>${esc(p.name)}<div class="muted">${esc(p.upc)}</div></td>
                  <td class="num">${p.units}</td><td class="num">${fmt$(p.net)}</td><td class="num">${fmt$(p.margin_amt)}</td>
                </tr>`).join("")}</tbody></table></div>
            </div>`).join("") || '<div class="placeholder"><h2>No sales in range</h2></div>';
          break;
        }
        case "stock": {
          const d = await API.get(`/api/reports/stock${branchQ()}`);
          const flat = [];
          d.forEach((dept) => dept.products.forEach((p) => flat.push([p.upc, p.name, dept.dept_name, p.on_hand, `$${p.retail_value}`, `$${p.cost_value}`, p.units_30])));
          lastData = { head: ["UPC", "Product", "Dept", "OnHand", "Retail", "Cost", "30d units"], rows: flat };
          const agg = (fn) => d.map(fn).reduce((a, b) => a + b, 0);
          res.innerHTML = `
            <div class="stats-row">
              <div class="stat"><span class="stat-val">${fmt$(agg((x) => x.retail_value))}</span><span class="stat-lbl">Retail value</span></div>
              <div class="stat"><span class="stat-val">${fmt$(agg((x) => x.cost_value))}</span><span class="stat-lbl">Cost value</span></div>
              <div class="stat"><span class="stat-val">${fmt$(agg((x) => x.gp_value))}</span><span class="stat-lbl">GP</span></div>
              <div class="stat"><span class="stat-val">${agg((x) => x.units)}</span><span class="stat-lbl">Units on hand</span></div>
            </div>
            ${d.map((dept) => `<div class="panel" style="margin-bottom:8px">
              <h4>${esc(dept.dept_name)} <span class="muted">· ${dept.items} items · retail ${fmt$(dept.retail_value)} · cost ${fmt$(dept.cost_value)}</span></h4>
              <div class="table-wrap"><table><thead><tr><th>UPC</th><th>Product</th><th class="num">OnHand</th><th class="num">Retail $</th><th class="num">Cost $</th><th class="num">30d units</th></tr></thead>
              <tbody>${dept.products.map((p) => `<tr><td class="muted">${esc(p.upc)}</td><td>${esc(p.name)}</td><td class="num">${p.on_hand}</td><td class="num">${fmt$(p.retail_value)}</td><td class="num">${fmt$(p.cost_value)}</td><td class="num">${p.units_30}</td></tr>`).join("")}
              </tbody></table></div></div>`).join("")}`;
          break;
        }
        case "stocktakes": {
          const d = await API.get("/api/reports/stocktakes");
          lastData = { head: ["Started", "Branch", "Status", "Count file", "Ticket file"], rows: d.map((r) => [r.started_at, r.branch_id ?? "", r.status, r.count_file ?? "", r.ticket_file ?? ""]) };
          res.innerHTML = d.length
            ? tbl(["Started", "Branch", "Status", "Count file", "Ticket file"], d.map((r) => `<tr>
                <td>${esc(r.started_at)}</td><td class="num">${r.branch_id ?? ""}</td>
                <td><span class="ok">${esc(r.status)}</span></td>
                <td class="muted">${esc((r.count_file || "").split("/").pop())}</td>
                <td class="muted">${esc((r.ticket_file || "").split("/").pop())}</td></tr>`).join(""))
            : '<div class="placeholder"><h2>No stocktake exports yet — export one from Stocktake</h2></div>';
          break;
        }
        case "receipts": {
          const d = await API.get(`/api/reports/receipts?from=${f}&to=${t}${branchQ()}`);
          lastData = { head: ["Supplier", "Goods-in", "Returns", "Net GRN", "AP invoiced", "Variance"], rows: d.map((r) => [r.supplier_code, `$${r.goods_in}`, `$${r.returns}`, `$${r.net_grn}`, `$${r.ap_invoiced}`, `$${r.variance}`]) };
          const withVal = d.filter((r) => r.goods_in || r.returns || r.ap_invoiced);
          res.innerHTML = withVal.length
            ? tbl(["Supplier", "Goods-in", "Returns", "Net GRN", "AP invoiced", "Variance"], withVal.map((r) => `<tr>
                <td>${esc(r.supplier_code)} <span class="muted">${esc(r.supplier_name)}</span></td>
                <td class="num">${fmt$(r.goods_in)}</td><td class="num">${fmt$(r.returns)}</td><td class="num">${fmt$(r.net_grn)}</td>
                <td class="num">${fmt$(r.ap_invoiced)}</td><td class="num">${fmt$(r.variance)}</td></tr>`).join(""))
            : '<div class="placeholder"><h2>No receipts or AP in range</h2></div>';
          break;
        }
        case "promo": {
          const d = await API.get(`/api/promotions/effectiveness?from=${f}&to=${t}${branchQ()}`);
          lastData = null;
          res.innerHTML = d && d.length
            ? tbl(["Promo", "Kind", "Sales", "Promo units", "Total units", "Uplift"], d.slice(0, 100).map((p) => `<tr>
                <td>${esc(p.description || p.id || "")}</td><td>${esc(p.kind || "")}</td>
                <td class="num">${fmt$(p.sales ?? p.revenue ?? 0)}</td><td class="num">${p.promo_units ?? p.units ?? 0}</td>
                <td class="num">${p.total_units ?? p.units ?? 0}</td><td class="num">${p.uplift_pct != null ? p.uplift_pct.toFixed(1) + "%" : "—"}</td></tr>`).join(""))
            : '<div class="placeholder"><h2>No promo data in range</h2></div>';
          break;
        }
      }
    } catch (e) { $("#rp-msg").className = "msg error"; $("#rp-msg").textContent = e.message; }
  }

  for (const b of el.querySelectorAll("[data-rp]")) {
    b.onclick = () => {
      el.querySelectorAll("[data-rp]").forEach((x) => x.classList.toggle("active", x === b));
      current = b.dataset.rp;
      run();
    };
  }
  $("#rp-csv").onclick = () => {
    if (!lastData) return;
    const lines = [lastData.head.join(","), ...lastData.rows.map((r) => r.map((c) => String(c).replace(/,/g, " ")).join(","))];
    download("report.csv", lines.join("\n"));
  };
  for (const id of ["rp-from", "rp-to"]) $(id).onchange = run;
  $("#rp-branch") && ($("#rp-branch").onchange = run);

  run();
}

function tbl(head, body) {
  return `<div class="table-wrap"><table><thead><tr>${head.map((h) => `<th>${h}</th>`).join("")}</tr></thead><tbody>${body}</tbody></table></div>`;
}
function branchOpts(server) {
  if (server.branch) return `<option>${server.branch}</option>`;
  return `<option value="ALL">All branches</option><option value="">—</option>`;
}
function iso(d) { return d.toISOString().slice(0, 10); }
function download(name, text) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], { type: "text/csv" }));
  a.download = name;
  a.click();
}
function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function fmt$(v) { return "$" + (v || 0).toLocaleString(undefined, { maximumFractionDigits: 0 }); }
