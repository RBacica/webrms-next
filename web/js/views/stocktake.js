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
      <label>Supplier</label>
      <select id="st-supplier"><option value="ALL">All suppliers</option></select>
      <label class="chk"><input type="checkbox" id="st-uncounted"> uncounted only</label>
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

  const $ = (id) => el.querySelector(id.startsWith("#") ? id : "#" + id);
  const qEl = () => $("st-q");
  async function loadSuppliers(deptId) {
    try {
      const s = await API.get(`/api/stocktake/suppliers-for-dept?dept=${deptId}`);
      const sel = $("st-supplier");
      const list = Array.isArray(s) ? s : (s.suppliers || []);
      sel.innerHTML = `<option value="ALL">All suppliers</option>` + list.map((x) => `<option value="${x.code}">${esc(x.name || x.label || x.code)}</option>`).join("");
    } catch { /* optional */ }
  }
  loadSuppliers("ALL");
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
      const ss = supplierFilter !== "ALL" ? `&supplier=${supplierFilter}` : "";
      const d = await API.get(`/api/stocktake/search?branch=${branch}${qs}${ds}${ss}`);
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

  let uncountedOnly = false;
  let supplierFilter = "ALL";
  function renderRows() {
    const rows = [...session.values()].filter((r) => {
      if (uncountedOnly && r.count !== null && r.count !== "") return false;
      if (supplierFilter !== "ALL" && r.supplier !== supplierFilter) return false;
      return true;
    });
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
      inp.addEventListener("keydown", (e) => {
        if (e.key !== "Enter") return;
        e.preventDefault();
        const all = Array.from(el.querySelectorAll(".st-count"));
        const idx = all.indexOf(inp);
        const next = all[idx + 1];
        if (next) next.focus();
        else qEl().focus();
      });
      inp.addEventListener("focus", () => inp.select());
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
  $("st-dept").onchange = () => { loadSuppliers($("st-dept").value); doSearch(); };
  $("st-supplier").onchange = () => { supplierFilter = $("st-supplier").value; renderRows(); };
  $("st-uncounted").onchange = () => { uncountedOnly = $("st-uncounted").checked; renderRows(); };

  function askDestination(rows) {
    return new Promise((resolve) => {
      const overlay = document.createElement("div");
      overlay.style.cssText = "position:fixed;inset:0;background:rgba(0,0,0,.6);display:flex;align-items:center;justify-content:center;z-index:100";
      overlay.innerHTML = `
        <div class="panel" style="min-width:380px">
          <h3>Export ${rows.length} rows</h3>
          <p class="muted" style="margin-bottom:12px">Where should the files go?</p>
          <div style="display:flex;flex-direction:column;gap:8px">
            <button id="ask-server">Save to server (data/output)</button>
            <button id="ask-client" class="secondary">Download to this device</button>
            <button id="ask-cancel" class="secondary">Cancel</button>
          </div>
        </div>`;
      document.body.appendChild(overlay);
      overlay.querySelector("#ask-server").onclick = () => { overlay.remove(); resolve("server"); };
      overlay.querySelector("#ask-client").onclick = () => { overlay.remove(); resolve("client"); };
      overlay.querySelector("#ask-cancel").onclick = () => { overlay.remove(); resolve(null); };
    });
  }

  $("st-save").onclick = async () => {
    const rows = saveRows();
    if (!rows.length) { msg("Nothing counted yet — enter counts first", "warn"); return; }
    const destination = await askDestination(rows);
    if (!destination) return;
    try {
      const r = await API.send("POST", "/api/stocktake/export", {
        destination,
        branch: SERVER.branch || undefined,
        rows,
      });
      if (destination === "client" && r.files) {
        for (const f of r.files) download(f.filename, f.content, "text/plain");
        msg(`Downloaded ${r.count_rows} count + ${r.ticket_rows} ticket lines (run recorded)`, "success");
      } else {
        msg(`Saved ${r.count_rows} count lines → ${r.count_file || r.ticket_file || "files"} (run recorded)`, "success");
      }
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
