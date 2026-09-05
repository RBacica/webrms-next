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

const VIEWS = ["overview", "reports", "items", "ordering", "stocktake", "payables", "promotions", "incoming"];
const VIEW_LABELS = {
  overview: "Overview", reports: "Reports", items: "Items", ordering: "Ordering", stocktake: "Stocktake",
  payables: "Payables", promotions: "Promotions", incoming: "Incoming PO",
};

let SERVER = { mode: "standalone", branch: null, author: false };
let currentView = "overview";
// Bump APP_VER on every web/ deploy — it versions the dynamic view imports so
// a browser can never run a stale cached view module against new HTML.
const APP_VER = 7;

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
  const el = document.getElementById("sync-status");
  try {
    const h = await API.get("/api/health");
    let txt = "";
    // connector freshness
    if (h.connector === "ok") txt = `connector ok`;
    else if (h.connector === "error") txt = `connector ERROR`;
    else txt = `connector off`;
    if (h.connector_age_secs !== null && h.connector_age_secs !== undefined && h.connector !== "disabled") {
      const a = h.connector_age_secs;
      const s = a < 90 ? `${a}s` : a < 5400 ? `${Math.round(a / 60)}m` : `${Math.round(a / 3600)}h`;
      txt += ` · poll ${s} ago`;
    }
    // replication lag (sync clients)
    if (h.replication && h.replication.role === "client") {
      const lag = h.replication.lag_minutes;
      if (lag === null) txt += ` · repl: never`;
      else if (lag < 1) txt += ` · repl <1m`;
      else if (lag < 60) txt += ` · repl ${lag}m`;
      else txt += ` · repl ${Math.round(lag / 60)}h`;
    }
    // fallback engagement badge
    if (h.fallback && h.fallback.engaged) {
      txt += ` · SNAPSHOT MODE`;
      el.className = "bad";
    } else {
      el.className = txt.includes("ERROR") ? "bad" : "ok";
    }
    el.textContent = txt;
  } catch { el.textContent = "sync: ?"; }
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
  // Cleanup contract: views register their intervals on el._timers so a
  // navigation away never leaves a poller running (incoming.js was stacking
  // one 60s interval per visit).
  if (el._timers) { el._timers.forEach((t) => clearInterval(t)); el._timers = []; }
  if (el._cleanup) { try { el._cleanup(); } catch (e) { /* ignore */ } el._cleanup = null; }
  el.innerHTML = '<div class="placeholder"><h2>Loading…</h2></div>';
  try {
    const mod = await import(`./views/${v}.js?v=${APP_VER}`);
    await mod.render(el, { API, SERVER });
  } catch (e) {
    el.innerHTML = `<div class="placeholder"><h2>View error</h2><p class="bad">${e.message}</p><pre style="text-align:left;font-size:11px;color:#c55">${e.stack || ""}</pre></div>`;
  }
  refreshSyncStatus();
}

window.addEventListener("hashchange", route);
(async function boot() {
  await loadMeta();
  route();
})();
