"use strict";
/* Jenova for DSM — window UI. Talks to the service through api.cgi using
   the DSM session (cookie + SynoToken). Polls only while visible. */

const LANGUAGES = [["en","English"],["en-gb","English (UK)"],["en-us","English (US)"],["af","Afrikaans"],["am","Amharic"],["ar","Arabic"],["az","Azerbaijani"],["bg","Bulgarian"],["bn","Bengali"],["cs","Czech"],["da","Danish"],["de","German"],["de-at","German (Austria)"],["de-ch","German (Switzerland)"],["el","Greek"],["es","Spanish"],["es-es","Spanish (Spain)"],["es-mx","Spanish (Mexico)"],["et","Estonian"],["fa","Persian/Farsi"],["fi","Finnish"],["fil","Filipino/Tagalog"],["fr","French"],["fr-ca","French (Canada)"],["gu","Gujarati"],["he","Hebrew"],["hi","Hindi"],["hr","Croatian"],["hu","Hungarian"],["id","Indonesian"],["is","Icelandic"],["it","Italian"],["it-ch","Italian (Switzerland)"],["ja","Japanese"],["ka","Georgian"],["km","Khmer"],["kn","Kannada"],["ko","Korean"],["ku","Kurdish"],["lo","Lao"],["lt","Lithuanian"],["lv","Latvian"],["ml","Malayalam"],["mn","Mongolian"],["ms","Malay"],["mt","Maltese"],["my","Burmese"],["nl","Dutch"],["nl-be","Dutch (Belgium)"],["no","Norwegian"],["or","Odia"],["pa","Punjabi"],["pl","Polish"],["pt","Portuguese"],["pt-br","Portuguese (Brazil)"],["pt-pt","Portuguese (Portugal)"],["ro","Romanian"],["ru","Russian"],["sk","Slovak"],["sl","Slovenian"],["sr","Serbian"],["sv","Swedish"],["sw","Swahili"],["ta","Tamil"],["te","Telugu"],["th","Thai"],["tr","Turkish"],["uk","Ukrainian"],["ur","Urdu"],["vi","Vietnamese"],["xh","Xhosa"],["zh","Chinese"],["zh-cn","Chinese (Simplified)"],["zh-tw","Chinese (Traditional)"],["zu","Zulu"]];

const PROVIDERS = [
  ["addic7ed", "Addic7ed"], ["gestdown", "Gestdown"], ["napiprojekt", "NapiProjekt"],
  ["opensubtitles", "OpenSubtitles"], ["opensubtitlescom", "OpenSubtitles.com"],
  ["podnapisi", "Podnapisi"], ["tvsubtitles", "TVSubtitles"],
];
const REFINERS = [["hash", "Hash"], ["metadata", "Metadata"], ["tmdb", "TMDB"], ["tvdb", "TVDB"]];
const PAGES = [
  ["watch", "Watch"], ["downloads", "Downloads"], ["language", "Language"],
  ["matching", "Matching"], ["config", "Config"], ["log", "Log"],
];

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

let token = "";
let status = null;
let config = null;
let page = "watch";
let pollTimer = null;

// ---------- API ----------

async function getToken() {
  try {
    const r = await fetch("/webman/login.cgi", { credentials: "same-origin" });
    const j = await r.json();
    token = j.SynoToken || "";
  } catch (e) { token = ""; }
}

async function api(route, body) {
  const opts = {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    headers: { "X-SYNO-TOKEN": token },
  };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const sep = route.includes("?") ? "&" : "?";
  const [r0, q0] = route.split("?");
  const url = "api.cgi?r=" + encodeURIComponent(r0) + (q0 ? "&" + q0 : "");
  const res = await fetch(url, opts);
  let data = {};
  try { data = await res.json(); } catch (e) { /* empty */ }
  if (res.status === 401) {
    showOverlay("Sign in needed", esc(data.error || "Your DSM session has expired. Sign in to DSM again."));
    throw new Error("auth");
  }
  if (res.status === 503) {
    showOverlay("Jenova isn't running", esc(data.error || "Start Jenova in Package Center."));
    throw new Error("down");
  }
  hideOverlay();
  if (!res.ok) throw new Error(data.error || "Request failed");
  return data;
}

function showOverlay(title, html) {
  $("#overlay-title").textContent = title;
  $("#overlay-text").innerHTML = html;
  $("#overlay").classList.add("open");
}
function hideOverlay() { $("#overlay").classList.remove("open"); }

// ---------- menu ----------

function buildMenu() {
  const nav = $("#menu");
  nav.innerHTML = PAGES.map(([id, label]) =>
    `<button class="menu-item" data-go="${id}"><svg class="hand"><use href="#hand"/></svg>${label}</button>`).join("");
  nav.addEventListener("click", (e) => {
    const b = e.target.closest("[data-go]");
    if (b) go(b.dataset.go);
  });
  go(location.hash.slice(1) || "watch");
}

function go(id) {
  if (!PAGES.some(([p]) => p === id)) id = "watch";
  page = id;
  $$(".menu-item").forEach((b) => b.classList.toggle("selected", b.dataset.go === id));
  $$(".page").forEach((p) => p.classList.toggle("active", p.dataset.page === id));
  history.replaceState(null, "", "#" + id);
  refresh();
}

// ---------- rendering ----------

function fmtTime(s) {
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, x = s % 60;
  return [h, m, x].map((n) => String(n).padStart(2, "0")).join(":");
}

function folderTag(f) {
  if (!f.readable && f.state !== "offline") return ["NO ACCESS", "bad"];
  switch (f.state) {
    case "watching": return ["WATCHING", "good"];
    case "polling": return ["POLLING", "good"];
    case "paused": return ["PAUSED", "warn"];
    case "offline": return ["OFFLINE", "bad"];
    default: return [f.state ? f.state.toUpperCase() : "…", "warn"];
  }
}

function permissionHelp(f) {
  const share = (f.path.split("/")[2]) || "the shared folder";
  return `Jenova can't ${f.readable ? "save subtitles in" : "read"} this folder. In DSM open <b>Control Panel → Shared Folder</b>, select <b>${esc(share)}</b>, click <b>Edit → Permissions</b>, switch the list to <b>System internal user</b>, and give <b>Jenova</b> <b>Read/Write</b>.`;
}

function renderStatus() {
  if (!status) return;
  $("#uptime").textContent = fmtTime(status.uptime);
  $("#subs-total").textContent = Number(status.subs_total).toLocaleString();

  const t = $("#watch-toggle");
  t.textContent = status.watch_enabled ? "WATCHING: ON" : "WATCHING: OFF";
  t.className = "big " + (status.watch_enabled ? "on" : "off");

  const box = $("#folders");
  if (!status.folders.length) {
    box.innerHTML = `<div class="empty"><h2>No folders yet</h2>Add your Movies or TV folder to start watching.</div>`;
  } else {
    box.innerHTML = status.folders.map((f) => {
      const [tag, cls] = folderTag(f);
      const letter = esc((f.name || "?").charAt(0).toUpperCase());
      const videos = f.videos ?? null, subs = f.with_subs ?? null;
      const pct = videos ? Math.round((subs / videos) * 100) : 0;
      const active = status.watch_enabled && f.readable && f.state !== "offline";
      const help = (!f.readable || !f.writable) && f.state !== "offline" ? `<div class="notice">${permissionHelp(f)}</div>`
        : f.error && f.state !== "paused" ? `<div class="notice">${esc(f.error)}</div>` : "";
      return `<div class="party">
        <div class="emblem ${active ? "" : "off"}">${letter}</div>
        <div class="body">
          <div class="title"><span class="name">${esc(f.name)}</span><span class="tag ${cls}">${tag}</span>
            <span class="actions"><button class="small" data-remove="${esc(f.path)}">Remove</button></span></div>
          <div class="path">${esc(f.path)}</div>
          <div class="stats">
            <span class="label">LV</span><span>${videos ?? "--"}</span>
            <span class="label" style="margin-left:10px">SUBS</span>
            <span class="bar" title="Videos that already have subtitles in your languages"><i style="width:${pct}%"></i></span>
            <span>${subs ?? "--"}/${videos ?? "--"}</span>
          </div>
          ${help}
        </div></div>`;
    }).join("");
  }
  $("#check-now").disabled = !status.watch_enabled || !status.folders.length || status.scanning;
  $("#scan-note").textContent = status.scanning ? "Counting videos…" : "";

  const s = status.subliminal || {};
  const stateEl = $("#subliminal-state");
  const help = $("#subliminal-help");
  help.style.display = "none";
  if (s.state === "ready") stateEl.innerHTML = `<span class="good">Ready</span> — Subliminal ${esc(s.version)}`;
  else if (s.state === "installing") stateEl.innerHTML = `<span class="warn">Installing…</span> ${esc(s.step || "")}`;
  else if (s.state === "checking") stateEl.innerHTML = `<span class="warn">Checking…</span>`;
  else if (s.state === "nopython") {
    stateEl.innerHTML = `<span class="bad">Python not found</span>`;
    help.style.display = "";
    help.innerHTML = `Install <b>Python 3.9</b> from <b>Package Center</b>, then click Reinstall.`;
  } else if (s.state === "error") {
    stateEl.innerHTML = `<span class="bad">Couldn't set up Subliminal</span>`;
    help.style.display = "";
    help.innerHTML = esc(s.message) + "<br>Check the NAS can reach the internet, then click Reinstall.";
  }
  $("#reinstall").disabled = s.state === "installing" || s.state === "checking";
  renderCleanup(status.cleanup || { state: "idle" });
  $("#about").textContent = `Jenova for DSM v${status.version}`;
  renderStatusBar();
}

function renderCleanup(c) {
  const text = $("#cleanup-text"), buttons = $("#cleanup-buttons");
  const set = (msg, cls, html) => { text.textContent = msg; text.className = cls; buttons.innerHTML = html; };
  switch (c.state) {
    case "searching": set("Searching your watched folders…", "warn", ""); break;
    case "deleting": set("Deleting…", "warn", ""); break;
    case "found":
      if (!c.count) set("No subtitle backups found.", "good", `<button class="small" data-cleanup="reset">OK</button>`);
      else set(`Found ${c.count.toLocaleString()} subtitle backup file(s).`, "warn",
        `<button data-cleanup="delete">Delete ${c.count.toLocaleString()} files</button> <button class="small" data-cleanup="reset">Cancel</button>`);
      break;
    case "done":
      set(c.failed ? `Deleted ${c.deleted} file(s); ${c.failed} couldn't be deleted (check Jenova's Read/Write permission).` : `Deleted ${c.deleted.toLocaleString()} file(s).`,
        "good", `<button class="small" data-cleanup="reset">OK</button>`);
      break;
    default:
      set("Older Windows versions saved copies like Movie.en.srt.bak next to your subtitles.", "dim",
        `<button data-cleanup="find" ${status && status.folders.length ? "" : "disabled"}>Find .bak files</button>`);
  }
}

function renderStatusBar() {
  const s = status;
  let text;
  const sub = s.subliminal || {};
  if (sub.state === "installing") text = "Setting up Subliminal: " + (sub.step || "…");
  else if (sub.state === "nopython") text = "Install Python 3.9 from Package Center so Jenova can download subtitles.";
  else if (sub.state === "error") text = "Subliminal couldn't be set up. See Config.";
  else if (s.running || s.queued) text = `Fetching subtitles: ${s.running} running, ${s.queued} waiting.`;
  else if (!s.watch_enabled) text = "Watching is off.";
  else if (!s.folders.length) text = "Add a folder to start watching.";
  else {
    const n = s.folders.filter((f) => f.state === "watching" || f.state === "polling").length;
    text = `Watching ${n} folder${n === 1 ? "" : "s"} for new videos.`;
    if (s.settling) text += ` ${s.settling} new file${s.settling === 1 ? "" : "s"} settling…`;
  }
  $("#statusbar").textContent = text;
}

function renderJobs(jobs) {
  const body = $("#jobs");
  $("#jobs-empty").style.display = jobs.length ? "none" : "";
  const label = {
    found: ["Found", "good"], none: ["None found", "warn"], failed: ["Failed", "bad"],
    running: ["Searching…", "warn"], queued: ["Waiting", "dim"], skipped: ["Already had subs", "dim"],
  };
  body.innerHTML = jobs.map((j) => {
    const [text, cls] = label[j.status] || [j.status, ""];
    const name = j.path.split("/").slice(-2).join(" / ");
    const langs = j.langs.length ? j.langs.map((l) => (LANGUAGES.find(([c]) => c === l) || [l, l])[1]).join(", ") : "";
    const msg = j.status === "failed" || j.status === "none" ? `<div class="dim small">${esc(j.message)}</div>` : "";
    return `<tr><td class="${cls}">${text}</td><td class="file" title="${esc(j.path)}">${esc(name)}${msg}</td><td>${esc(langs)}</td><td class="when">${esc(j.time.slice(5, 16))}</td></tr>`;
  }).join("");
}

function renderLog(lines) {
  const box = $("#log");
  const atBottom = box.scrollTop + box.clientHeight >= box.scrollHeight - 20;
  box.innerHTML = lines.map((l) => {
    const t = l.slice(11, 19), rest = l.slice(20);
    return `<div class="log-line"><span class="t">${esc(t)}</span><span>${esc(rest)}</span></div>`;
  }).join("") || `<div class="empty">No activity yet.</div>`;
  if (atBottom) box.scrollTop = box.scrollHeight;
}

// ---------- settings ----------

function buildChecklist(el, items, key) {
  el.innerHTML = items.map(([code, name]) =>
    `<label class="check" data-name="${esc(name.toLowerCase())} ${esc(code)}"><input type="checkbox" data-list="${key}" value="${code}"><span class="box"></span>${esc(name)}</label>`).join("");
}

function applyConfigToForm() {
  $$("[data-key]").forEach((el) => {
    const v = config[el.dataset.key];
    if (el.type === "checkbox") el.checked = !!v; else el.value = v ?? "";
  });
  $$("[data-list]").forEach((el) => { el.checked = (config[el.dataset.list] || []).includes(el.value); });
  $("#os-password").placeholder = config.has_password ? "•••••••• (saved)" : "";
  $("#lang-count").textContent = `${config.languages.length} selected`;
}

let saveTimer = null;
function queueSave(changes) {
  Object.assign(pending, changes);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(flushSave, 400);
}
let pending = {};
async function flushSave() {
  const changes = pending; pending = {};
  if (!Object.keys(changes).length) return;
  try {
    config = await api("config", changes);
    applyConfigToForm();
    $$(".page.active .saved").forEach((s) => { s.classList.add("show"); setTimeout(() => s.classList.remove("show"), 1200); });
    refreshStatus();
  } catch (e) {
    if (e.message !== "auth" && e.message !== "down") alert(e.message);
  }
}

function wireSettings() {
  buildChecklist($("#languages"), LANGUAGES, "languages");
  buildChecklist($("#providers"), PROVIDERS, "providers");
  buildChecklist($("#refiners"), REFINERS, "refiners");
  document.addEventListener("change", (e) => {
    const el = e.target;
    if (el.dataset.key) {
      const v = el.type === "checkbox" ? el.checked : el.type === "number" ? Number(el.value) : el.value;
      queueSave({ [el.dataset.key]: v });
    } else if (el.dataset.list) {
      const key = el.dataset.list;
      const values = $$(`[data-list="${key}"]`).filter((x) => x.checked).map((x) => x.value);
      if (key === "languages" && !values.length) { el.checked = true; return; }
      queueSave({ [key]: values });
    } else if (el.id === "os-password" && el.value) {
      queueSave({ opensubtitles_password: el.value });
      el.value = "";
    }
  });
  $("#clear-password").onclick = () => queueSave({ clear_password: true });
  $("#lang-filter").oninput = (e) => {
    const q = e.target.value.trim().toLowerCase();
    $$("#languages label").forEach((l) => { l.style.display = !q || l.dataset.name.includes(q) ? "" : "none"; });
  };
}

// ---------- folder picker ----------

let pickerPath = "";
async function openPicker(path = "") {
  try {
    const data = await api("browse?path=" + encodeURIComponent(path));
    pickerPath = data.path;
    $("#picker-crumb").textContent = data.path || "Shared folders";
    $("#picker-up").disabled = data.parent === null || data.parent === undefined;
    $("#picker-up").dataset.parent = data.parent ?? "";
    $("#picker-choose").disabled = !data.path;
    const list = $("#picker-list");
    list.innerHTML = data.entries.map((e) =>
      `<div class="picker-item" data-path="${esc(e.path)}">📁 <span>${esc(e.name)}</span>${e.readable ? "" : ' <span class="bad small">no access</span>'}${e.volume ? `<span class="vol">${esc(e.volume)}</span>` : ""}</div>`).join("")
      || `<div class="empty">No subfolders</div>`;
    $("#picker-note").style.display = "none";
    $("#picker").classList.add("open");
  } catch (e) {
    if (e.message !== "auth") alert(e.message);
  }
}

function wirePicker() {
  $("#add-folder").onclick = () => openPicker("");
  $("#picker-list").onclick = (e) => {
    const item = e.target.closest("[data-path]");
    if (item) openPicker(item.dataset.path);
  };
  $("#picker-up").onclick = (e) => openPicker(e.target.dataset.parent || "");
  $("#picker-cancel").onclick = () => $("#picker").classList.remove("open");
  $("#picker-choose").onclick = async () => {
    try {
      await api("folders/add", { path: pickerPath });
      $("#picker").classList.remove("open");
      refreshStatus();
    } catch (e) {
      const note = $("#picker-note");
      note.textContent = e.message;
      note.style.display = "";
    }
  };
}

// ---------- actions ----------

function wireActions() {
  $("#watch-toggle").onclick = () => queueSave({ watch_enabled: !status.watch_enabled });
  $("#check-now").onclick = async () => { await api("check", {}); refreshStatus(); };
  $("#folders").onclick = async (e) => {
    const b = e.target.closest("[data-remove]");
    if (b && confirm(`Stop watching ${b.dataset.remove}?`)) {
      await api("folders/remove", { path: b.dataset.remove });
      refreshStatus();
    }
  };
  $("#clear-jobs").onclick = async () => { await api("jobs/clear", {}); refresh(); };
  $("#reinstall").onclick = async () => { await api("subliminal/reinstall", {}); refreshStatus(); };
  $("#cleanup-buttons").onclick = async (e) => {
    const b = e.target.closest("[data-cleanup]");
    if (!b) return;
    await api("cleanup/" + b.dataset.cleanup, {});
    refreshStatus();
    setTimeout(refreshStatus, 800);
  };
}

// ---------- polling ----------

async function refreshStatus() {
  try { status = await api("status"); renderStatus(); } catch (e) { /* shown via overlay */ }
}

async function refresh() {
  await refreshStatus();
  try {
    if (page === "downloads") renderJobs((await api("jobs")).jobs);
    if (page === "log") renderLog((await api("log")).lines);
  } catch (e) { /* ignore */ }
}

function schedule() {
  clearTimeout(pollTimer);
  if (document.visibilityState !== "visible") return;
  pollTimer = setTimeout(async () => { await refresh(); schedule(); }, 3000);
}

document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") { refresh(); } schedule(); });

(async function init() {
  await getToken();
  buildMenu();
  wireSettings();
  wirePicker();
  wireActions();
  try {
    config = await api("config");
    applyConfigToForm();
  } catch (e) { /* overlay */ }
  await refresh();
  schedule();
})();
