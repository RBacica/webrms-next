/* Items maintenance (W6): search (active + inactive), edit fields, clone to a
   new UPC (user convention: retire old with SKU=OLD_<new>, alt barcode = old),
   ETL patch export for Infinity. Writes are Head Office only. */
"use strict";
export async function render(el, { API, SERVER }) {
  const author = SERVER.author || SERVER.mode === "hos";
  el.innerHTML = `
  <div class="panel">
    <div class="toolbar" style="flex-wrap:wrap">
      <input type="search" id="it-q" placeholder="Search UPC / SKU / description / barcode…" style="min-width:280px">
      <button id="it-search" class="secondary">Search</button>
      <label class="chk"><input type="checkbox" id="it-inactive"> include inactive</label>
      <label class="chk"><input type="checkbox" id="it-nonstock"> non-stock</label>
      <div style="margin-left:auto"></div>
      <div id="it-editor" class="muted" style="font-size:13px"></div>
    </div>
    <div class="toolbar" style="flex-wrap:wrap;margin-top:8px">
      <select id="it-dept" style="max-width:220px"><option value="">All departments</option></select>
      <select id="it-sub" style="max-width:200px"><option value="">All sub-depts</option></select>
      <select id="it-class" style="max-width:160px"><option value="">All classes</option></select>
      <select id="it-supplier" style="max-width:240px"><option value="">All suppliers</option></select>
      <select id="it-disc" style="max-width:140px"><option value="">All disc groups</option></select>
      <select id="it-parent" style="max-width:170px">
        <option value="">All items</option>
        <option value="parent">Parent items only</option>
        <option value="child">Child items only</option>
      </select>
    </div>
    <div id="it-msg" class="msg"></div>
  </div>
  <div id="it-results" class="panel"><div class="placeholder"><h2>Search for an item to edit or clone</h2></div></div>`;

  const $ = (id) => el.querySelector(id.startsWith("#") ? id : "#" + id);
  const msg = (t, c) => { $("it-msg").className = c ? `msg ${c}` : "msg"; $("it-msg").textContent = t; };
  let current = null;

  // load filter facets
  try {
    const f = await API.get("/api/items/facets");
    const fillSel = (id, opts, nameOf) => {
      (opts || []).forEach((o) => {
        const opt = document.createElement("option");
        opt.value = o.id !== undefined ? String(o.id) : String(o);
        opt.textContent = nameOf ? nameOf(o) : String(o);
        $(id).appendChild(opt);
      });
    };
    fillSel("it-dept", f.departments, (o) => o.name);
    fillSel("it-sub", f.sub_departments);
    fillSel("it-class", f.classes);
    fillSel("it-supplier", f.suppliers, (o) => `${o.code} · ${o.name}`);
    fillSel("it-disc", f.disc_groups);
  } catch { /* facets are non-fatal */ }

  async function doSearch() {
    const q = $("it-q").value.trim();
    const incl = $("it-inactive").checked;
    const ns = $("it-nonstock").checked;
    const f = new URLSearchParams({ q });
    if (incl) f.set("include_inactive", "true");
    if (ns) f.set("non_stock", "true");
    const dept = $("it-dept").value, sub = $("it-sub").value, cls = $("it-class").value,
          sup = $("it-supplier").value, disc = $("it-disc").value, par = $("it-parent").value;
    if (dept) f.set("department", dept);
    if (sub) f.set("sub", sub);
    if (cls) f.set("class", cls);
    if (sup) f.set("supplier", sup);
    if (disc) f.set("disc_group", disc);
    if (par) f.set("parent", par);
    msg("Searching…");
    try {
      const d = await API.get(`/api/items/search?${f.toString()}`);
      const items = d.items || [];
      $("it-results").innerHTML = items.length
        ? `<div class="table-wrap"><table>
        <thead><tr><th>UPC</th><th>SKU</th><th>Description</th><th>Supplier</th><th class="num">Cost</th><th class="num">Price1</th><th>State</th><th></th></tr></thead>
        <tbody>${items.map((i) => `<tr data-upc="${esc(i.upc)}" class="${i.is_active ? "" : "row-inactive"}">
          <td class="muted">${esc(i.upc)}</td><td class="muted">${esc(i.sku || "")}</td>
          <td>${esc(i.description || "")}${i.overridden ? ' <span class="badge-ish">edited</span>' : ""}${i.non_stock ? ' <span class="muted">(non-stock)</span>' : ""}</td>
          <td class="muted">${esc(i.supplier_code || "")}${i.disc_group ? ` · DG ${esc(i.disc_group)}` : ""}</td>
          <td class="num">${i.cost}</td><td class="num">${i.price1}</td>
          <td>${i.is_active ? '<span class="ok">active</span>' : '<span class="muted">inactive</span>'}</td>
          <td class="num"><button class="secondary it-open">Edit</button></td>
        </tr>`).join("")}</tbody></table></div>`
        : '<div class="placeholder"><h2>No items found</h2></div>';
      for (const b of el.querySelectorAll(".it-open")) {
        b.onclick = () => openEditor(b.closest("tr").dataset.upc);
      }
      msg(`${items.length} items`);
    } catch (e) { msg(e.message, "error"); }
  }

  async function openEditor(upc) {
    try {
      const i = await API.get(`/api/items/${upc}`);
      current = i;
      $("it-editor").innerHTML = `
        <div class="panel" style="margin-top:8px">
        <b>${esc(i.description || i.upc)}</b> <span class="muted">${esc(i.upc)} · sup ${esc(i.supplier_code || "—")}</span>
        <div class="toolbar" style="flex-wrap:wrap;margin-top:6px">
          <label>Description</label><input id="ie-desc" value="${esc(i.description || "")}" ${author ? "" : "disabled"}>
          <label>Cost</label><input id="ie-cost" type="number" step="0.01" value="${i.cost}" ${author ? "" : "disabled"} style="width:90px">
          <label>Price1</label><input id="ie-price1" type="number" step="0.01" value="${i.price1}" ${author ? "" : "disabled"} style="width:90px">
          <label>Pack</label><input id="ie-pack" type="number" step="1" value="${i.pack_units}" ${author ? "" : "disabled"} style="width:70px">
          <label class="chk"><input type="checkbox" id="ie-active" ${i.is_active ? "checked" : ""} ${author ? "" : "disabled"}> active</label>
          ${author ? `<button id="ie-save" class="secondary">Save edit</button>
          <button id="ie-clone" class="secondary" title="New UPC (old is retired SKU=OLD_<new>, history carried)">Clone →</button>
          <input id="ie-newupc" placeholder="new UPC" style="width:140px">` : ""}
        </div></div>`;
      if (author) {
        $("#ie-save").onclick = async () => {
          const fields = {};
          fields.description = $("#ie-desc").value;
          fields.cost = +$("#ie-cost").value;
          fields.price1 = +$("#ie-price1").value;
          fields.pack_units = +$("#ie-pack").value;
          fields.is_active = $("#ie-active").checked;
          try {
            const r = await API.send("POST", "/api/items/edit", { upc: i.upc, fields });
            msg(`Saved edit on ${i.upc} (fields protected from connector)`, "success");
            await etlDownload("edit", i.upc);
          } catch (e) { msg(e.message, "error"); }
        };
        $("#ie-clone").onclick = async () => {
          const newUPC = $("#ie-newupc").value.trim();
          if (!newUPC) { msg("Enter the new UPC", "warn"); return; }
          try {
            const r = await API.send("POST", "/api/items/clone", {
              from_upc: i.upc, new_upc: newUPC,
              fields: { is_active: true },
            });
            msg(`Cloned ${i.upc} → ${newUPC} (old SKU = OLD_${newUPC}, inactive; history carried)`, "success");
            await etlDownload("clone", i.upc, newUPC);
            openEditor(newUPC);
          } catch (e) { msg(e.message, "error"); }
        };
      }
    } catch (e) { msg(e.message, "error"); }
  }

  async function etlDownload(kind, upc, newUPC) {
    try {
      const body = kind === "edit" ? { kind, upc } : { kind, from_upc: upc, new_upc: newUPC };
      const r = await API.send("POST", "/api/items/etl", body);
      msg(`Edit saved — ETL patch ${r.filename} ready for Infinity import (${r.path})`, "success");
      // offer download of the generated .xlsx
      const dl = confirm(`Download ${r.filename} for the Infinity Items import?`);
      if (dl) {
        const raw = await fetch(`/api/items/patch/${encodeURIComponent(r.filename)}`);
        if (raw.ok) {
          const blob = await raw.blob();
          download(r.filename, blob, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        } else msg("Could not download the patch file", "error");
      }
    } catch (e) { msg("Saved (ETL export failed: " + e.message + ")", "error"); }
  }

  $("it-search").onclick = doSearch;
  $("it-q").onkeydown = (e) => { if (e.key === "Enter") doSearch(); };
  $("it-inactive").onchange = doSearch;
  doSearch();
}
function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function download(name, content, type) {
  const blob = content instanceof Blob ? content : new Blob([content], { type });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
