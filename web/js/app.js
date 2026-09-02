/* WebRMS-Next SPA shell — hash router + api helper + status bar. */
"use strict";

const API = {
  async get(path) {
    const r = await fetch(path);
    if (!r.ok) throw new Error(`${r.status}: ${(await r.text()).slice(0, 300)}`);
    return r.json();
  },
  async send(method, path, body) {
    const r = await fetch(path, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!r.ok) throw new Error(`${r.status}: ${(await r.text()).slice(0, 300)}`);
    return r.json();
  },
};

const VIEWS = ["overview", "ordering", "stocktake", "payables", "promotions", "incoming"];
const VIEW_LABELS = {
  overview: "Overview", ordering: "Ordering", stocktake: "Stocktake",
  payables: "Payables", promotions: "Promotions", incoming: "Incoming PO",
};

let SERVER = { mode: "standalone", branch: null, author: false };
let currentView = "overview";

async function loadMeta() {
  try {
    const m = await API.get("/api/mode");
    SERVER.mode = m.mode;
    SERVER.author = !!m.author;
    SERVER.branch = m.branch_id;
    const badge = document.getElementById("mode-badge");
    badge.textContent = m.mode.toUpperCase();
    badge.className = "badge " + (m.mode === "bos" ? "bos" : m.mode === "remote-hos" ? "remote" : m.mode === "hos" ? "hos" : "unknown");
    document.getElementById("branch-info").textContent = m.branch_id ? `branch ${m.branch_id}` : "";
    document.getElementById("db-status").textContent = "DB: " + (m.db_ok ? "ok" : "DOWN");
    document.getElementById("db-status").className = m.db_ok ? "ok" : "bad";
  } catch (e) {
    document.getElementById("db-status").textContent = "DB: unreachable";
    document.getElementById("db-status").className = "bad";
  }
  refreshSyncStatus();
}

async function refreshSyncStatus() {
  try {
    const s = await API.get("/api/sync/status");
    const el = document.getElementById("sync-status");
    el.textContent = `sync: ${s.enabled ? `on · tick ${s.tick_count}` : "off"}`;
  } catch { /* standalone — no sync */ }
}

function nav() {
  const nav = document.getElementById("nav");
  nav.innerHTML = "";
  for (const v of VIEWS) {
    const b = document.createElement("button");
    b.textContent = VIEW_LABELS[v];
    b.className = v === currentView ? "active" : "";
    b.onclick = () => { location.hash = v; };
    nav.appendChild(b);
  }
}

async function route() {
  const v = (location.hash || "#overview").slice(1);
  if (!VIEWS.includes(v)) { location.hash = "overview"; return; }
  currentView = v;
  nav();
  const el = document.getElementById("view");
  el.innerHTML = '<div class="placeholder"><h2>Loading…</h2></div>';
  try {
    const mod = await import(`./views/${v}.js`);
    await mod.render(el, { API, SERVER });
  } catch (e) {
    el.innerHTML = `<div class="placeholder"><h2>View error</h2><p class="bad">${e.message}</p></div>`;
  }
  refreshSyncStatus();
}

window.addEventListener("hashchange", route);
(async function boot() {
  await loadMeta();
  route();
})();
