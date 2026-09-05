/* Stocktake count session (G-6 barcode UX): search/barcode → count grid w/
   variance → Save (server .txt/.qry + run recorded) / Export (client download).
   Barcode quick-add focuses stays on the input; counts accumulate in the
   session until Reset. */
"use strict";

export async function render(el, { API, SERVER }) {
  const depts = await API.get("/api/stocktake/departments");
  const branch = SERVER.branch || "";
  const branchLabel = branch ? `Branch ${branch}` : "All branches";
  el.innerHTML = `
  <div class="panel">
    <div class="toolbar" style="flex-wrap:wrap">
      <span class="stat-lbl">${branchLabel}</span>
      <label>Dept</label>
      <select id="st-dept"><option value="ALL">All departments</option>
        ${depts.map((d) => `<option value="${d.id}">${d.label}</option>`).join("")}</select>
      <input type="search" id="st-q" placeholder="Scan barcode or search (UPC / name)…" autofocus style="min-width:280px">
      <button id="st-search" class="secondary">Search</button>
      <div class="btn-group" style="margin-left:auto">
        <button id="st-save">Save counts → server</button>
        <button id="st-export" class="secondary" title="Download the .txt/.qry for Infinity Save/Restore">Download files</button>
        <button id="st-clear" class="secondary">Reset session</button>
      </div>
    </div>
    <div class="stats-row" id="st-stats" style="display:none">
      <div class="stat"><span class="stat-val" id="st-n">0</span><span class="stat-lbl">counted</span></div>
      <div class="stat"><span class="stat-val" id="st-total">0</span><span class="stat-lbl">total units</span></div>
      <div class="stat"><span class="stat-val" id="st-vary">0</span><span class="stat-lbl">lines w/ variance</span></div>
    </div>
    <div id="st-msg" class="msg"></div>
  </div>
  <div id="st-results" class="panel"><div class="placeholder"><h2>Scan a barcode to start counting</h2></div></div>`;

  const $ = (id) => el.querySelector(id);
  const qEl = () => $("st-q");
  const msg = (t, cls) => { $("st-msg").className = cls ? `msg ${cls}` : "msg"; $("st-msg").textContent = t; };
  // session: upc → row (description/department/supplier/soh filled on add)
  const session = new Map();
  let lastDept = "";

  async function addUpc(upc) {
    try {
      const d = await API.get(`/api/stocktake/refresh-upc?upc=${encodeURIComponent(upc)}${branch ? `&branch=${branch}` : ""}`);
      if (d.error) { msg(d.error, "error"); return; }
      // find the item's description/department via a targeted search
      const s = await API.get(`/api/stocktake/search?branch=${branch}&q=${encodeURIComponent(upc)}`);
      const it = (s.items || []).find((i) => i.upc === upc);
      if (!it) { msg(`Item ${upc} not found`, "error"); return; }
      session.set(upc, {
        upc,
        description: it.description,
        department: it.department,
        supplier: it.supplier,
        stock_on_hand: d.stock_on_hand ?? d.qty ?? it.stock_on_hand ?? 0,
        count: null,
      });
      renderRows();
      msg(`Added ${it.description} — enter count`, "success");
    } catch (e) { msg(e.message, "error"); }
  }

  async function doSearch() {
    const q = qEl().value.trim();
    const dept = $("st-dept").value;
    if (!q && dept === "ALL") { msg("Type a search or scan a barcode", "warn"); return; }
    msg("Searching…");
    try {
      const qs = q ? `&q=${encodeURIComponent(q)}` : "";
      const ds = dept !== "ALL" ? `&dept=${dept}` : "";
      const d = await API.get(`/api/stocktake/search?branch=${branch}${qs}${ds}`);
      const items = d.items || [];
      for (const it of items) {
        if (!session.has(it.upc)) {
          session.set(it.upc, {
            upc: it.upc,
            description: it.description,
            department: it.department || "",
            supplier: it.supplier || "",
            stock_on_hand: it.stock_on_hand ?? 0,
            count: null,
          });
        }
      }
      lastDept = dept;
      renderRows();
      msg(`${items.length} items added to the count session`);
      qEl().value = "";
      qEl().focus();
    } catch (e) { msg(e.message, "error"); }
  }

  function renderRows() {
    const rows = [...session.values()];
    $("st-stats").style.display = rows.length ? "" : "none";
    const counted = rows.filter((r) => r.count !== null && r.count !== "");
    $("st-n").textContent = counted.length;
    $("st-total").textContent = counted.reduce((a, r) => a + (+r.count || 0), 0);
    $("st-vary").textContent = counted.filter((r) => r.count !== r.stock_on_hand).length;
    $("st-results").innerHTML = rows.length
      ? `<div class="table-wrap"><table>
      <colgroup><col class="c-desc"><col class="c-num"><col class="c-num"><col class="c-num"><col class="c-num"></colgroup>
      <thead><tr><th>Description</th><th class="num">SOH</th><th class="num">Count</th><th class="num">Variance</th><th class="num"></th></tr></thead>
      <tbody>${rows.map((r) => {
        const v = r.count === null || r.count === "" ? null : +r.count - r.stock_on_hand;
        const cls = v !== null && v !== 0 ? "has-variance" : "";
        return `<tr class="${cls}" data-upc="${esc(r.upc)}">
          <td>${esc(r.description)}<div class="muted">${esc(r.upc)}</div></td>
          <td class="num">${r.stock_on_hand}</td>
          <td class="num"><input type="number" min="0" step="1" value="${r.count ?? ""}" class="st-count" style="width:80px" autocomplete="off"></td>
          <td class="num st-var">${v === null ? "—" : v}</td>
          <td class="num"><button class="secondary st-del" title="Remove">✕</button></td>
        </tr>`; }).join("")}</tbody></table></div>`
      : '<div class="placeholder"><h2>Session empty — scan or search to add items</h2></div>';
    for (const inp of el.querySelectorAll(".st-count")) {
      inp.onchange = () => {
        const row = session.get(inp.closest("tr").dataset.upc);
        row.count = inp.value === "" ? null : +inp.value;
        renderRows();
        qEl().focus();
      };
    }
    for (const b of el.querySelectorAll(".st-del")) {
      b.onclick = () => {
        session.delete(b.closest("tr").dataset.upc);
        renderRows();
        qEl().focus();
      };
    }
  }

  function saveRows() {
    const rows = [];
    for (const r of session.values()) {
      if (r.count === null || r.count === "") continue;
      rows.push({
        upc: r.upc,
        description: r.description || r.upc,
        department: r.department || "",
        supplier: r.supplier || "",
        stock_on_hand: r.stock_on_hand,
        count: +r.count,
        variance: +r.count - r.stock_on_hand,
        has_ticket: false,
        ticket_qty: 0,
      });
    }
    return rows;
  }

  $("st-search").onclick = doSearch;
  qEl().onkeydown = (e) => {
    if (e.key !== "Enter") return;
    const q = qEl().value.trim();
    if (/^[\d]{6,}$/.test(q)) { addUpc(q); qEl().value = ""; qEl().focus(); }
    else doSearch();
  };
  $("st-dept").onchange = doSearch;

  $("st-save").onclick = async () => {
    const rows = saveRows();
    if (!rows.length) { msg("Nothing counted yet — enter counts first", "warn"); return; }
    try {
      const r = await API.send("POST", "/api/stocktake/export", {
        destination: "server",
        branch: SERVER.branch || undefined,
        rows,
      });
      msg(`Saved ${r.count_rows} count lines → ${r.count_file || r.ticket_file || "files"} (run recorded)`, "success");
      session.clear();
      renderRows();
      qEl().focus();
    } catch (e) { msg(e.message, "error"); }
  };

  $("st-export").onclick = async () => {
    const rows = saveRows();
    if (!rows.length) { msg("Nothing counted yet — enter counts first", "warn"); return; }
    try {
      const r = await API.send("POST", "/api/stocktake/export", {
        destination: "client",
        branch: SERVER.branch || undefined,
        rows,
      });
      for (const f of r.files || []) download(f.filename, f.content, f.filename.endsWith(".qry") ? "text/plain" : "text/plain");
      msg(`Downloaded ${r.count_rows} count + ${r.ticket_rows} ticket lines (run recorded)`, "success");
    } catch (e) { msg(e.message, "error"); }
  };

  $("st-clear").onclick = () => {
    session.clear();
    renderRows();
    qEl().value = "";
    msg("Session reset");
    qEl().focus();
  };

  qEl().focus();
}

function download(name, content, type) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([content], { type }));
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
