/* Incoming PO — W5 lifecycle with status tags + HoS delete. */
"use strict";
export async function render(el, { API, SERVER }) {
  const load = async () => {
    el.innerHTML = '<div class="panel"><div class="placeholder"><h2>Loading…</h2></div></div>';
    try {
      const d = await API.get("/api/sync/incoming-po");
      const rows = d.incoming || [];
      el.innerHTML = `
      <div class="panel">
        <div class="stats-row">
          <div class="stat"><span class="stat-val">${rows.length}</span><span class="stat-lbl">PO files</span></div>
          <div class="stat"><span class="stat-val">${rows.filter((r) => r.status === "receipted").length}</span><span class="stat-lbl">Receipted</span></div>
          <div class="stat"><span class="stat-val">${rows.filter((r) => r.status === "waiting_import").length}</span><span class="stat-lbl">Waiting import</span></div>
        </div>
        <div class="table-wrap"><table class="in-po-table">
          <colgroup><col class="in-col-file"><col class="in-col-supplier"><col class="in-col-placed"><col class="in-col-meta"><col class="in-col-status"><col class="in-col-actions"></colgroup>
          <thead><tr><th>File</th><th>Supplier</th><th class="num">Placed</th><th class="num">POID</th><th>Status</th><th class="in-th-actions">Actions</th></tr></thead>
          <tbody>${rows.map((r) => `<tr>
            <td class="in-cell-file">${esc(r.filename)}</td>
            <td class="in-cell-supplier">${esc(r.supplier_code) || "—"}</td>
            <td class="num">${esc(r.placed_at || "")}</td>
            <td class="num">${r.poid}</td>
            <td class="in-cell-status">${statusTag(r.status)}</td>
            <td class="in-cell-actions">${SERVER.mode === "hos" ? `<button class="secondary in-del" data-f="${esc(r.filename)}">Delete</button>` : ""}</td>
          </tr>`).join("")}</tbody></table></div>
      </div>`;
      for (const b of el.querySelectorAll(".in-del")) {
        b.onclick = async () => {
          if (!confirm(`Delete ${b.dataset.f}?`)) return;
          try {
            await API.send("DELETE", `/api/sync/incoming-po?filename=${encodeURIComponent(b.dataset.f)}`);
            load();
          } catch (e) { alert(e.message); }
        };
      }
    } catch (e) {
      el.innerHTML = `<div class="panel"><p class="bad">${e.message}</p></div>`;
    }
  };
  load();
  // register the poll interval with the router so it is cleared on nav away
  el._timers = el._timers || [];
  el._timers.push(setInterval(load, 60000));
}

function statusTag(s) {
  const cls = s === "receipted" ? "ok" : s === "pending_receipt" ? "warn" : "muted";
  return `<span class="${cls}">${s}</span>`;
}
function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
