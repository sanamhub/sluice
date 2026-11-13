"use strict";
// The page drives the local daemon and never carries file bytes. Every call carries the key the
// daemon handed out in the URL fragment; text a peer chose is only ever set as textContent.

const $ = (id) => document.getElementById(id);

// ---- formatting ---------------------------------------------------------------------------------

function bytes(n) {
  n = Number(n || 0);
  const units = ["bytes", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return i === 0 ? `${n} ${n === 1 ? "byte" : "bytes"}` : `${n.toFixed(n < 10 ? 1 : 0)} ${units[i]}`;
}
function count(n, one, many) { n = Number(n || 0); return `${n} ${n === 1 ? one : many}`; }
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

// ---- send ---------------------------------------------------------------------------------------

const send = { roots: [], root: null, path: [], picked: new Map() };

async function loadRoots() {
  const { roots } = await call("/roots");
  send.roots = roots;
  const sel = $("recv-root");
  sel.replaceChildren(...roots.map((r) => { const o = el("option", "", r.name); o.value = r.name; return o; }));
  if (roots.length === 0) {
    $("entries").replaceChildren(el("li", "empty-row", "The daemon has no folders to work in. Start it with --root FOLDER."));
    $("recv-go").disabled = true;
    return;
  }
  openFolder(roots[0].name, []);
}

async function openFolder(root, path) {
  send.root = root;
  send.path = path;
  renderCrumbs();
  const list = $("entries");
  try {
    const { entries } = await call("/entries", { root, path: path.join("/") });
    if (entries.length === 0) { list.replaceChildren(el("li", "empty-row", "This folder is empty.")); return; }
    list.replaceChildren(...entries.map(entryRow));
  } catch (e) {
    list.replaceChildren(el("li", "empty-row", e.message));
  }
}

function entryRow(entry) {
  const rel = [...send.path, entry.name].join("/");
  const key = `${send.root}\u0000${rel}`;
  const li = el("li", entry.kind);
  const label = el("label");
  const box = el("input");
  box.type = "checkbox";
  box.checked = send.picked.has(key);
  box.addEventListener("change", () => {
    if (box.checked) send.picked.set(key, { root: send.root, path: rel });
    else send.picked.delete(key);
    renderPicked();
  });
  label.append(box, el("span", "name", entry.name));
  if (entry.kind === "file") label.append(el("span", "size", bytes(entry.size)));
  li.append(label);
  if (entry.kind === "folder") {
    const open = el("button", "open", "Open");
    open.type = "button";
    open.setAttribute("aria-label", `Open ${entry.name}`);
    open.addEventListener("click", () => openFolder(send.root, [...send.path, entry.name]));
    li.append(open);
  }
  return li;
}

function renderCrumbs() {
  const crumbs = $("crumbs");
  const parts = [];
  if (send.roots.length > 1) {
    const sel = el("select");
    sel.setAttribute("aria-label", "Folder");
    for (const r of send.roots) {
      const o = el("option", "", r.name);
      o.value = r.name;
      o.selected = r.name === send.root;
      sel.append(o);
    }
    sel.addEventListener("change", () => openFolder(sel.value, []));
    sel.style.width = "auto";
    parts.push(sel);
  }
  // Crumb i opens the folder made of the first i path parts; crumb 0 is the root itself, shown as a
  // button only when there is no root chooser to stand for it.
  const trail = [send.root, ...send.path];
  trail.forEach((name, i) => {
    if (i === 0 && send.roots.length > 1) return;
    if (i > 0) parts.push(el("span", "", "/"));
    const b = el("button", "", name);
    b.type = "button";
    if (i === trail.length - 1) b.setAttribute("aria-current", "true");
    else b.addEventListener("click", () => openFolder(send.root, send.path.slice(0, i)));
    parts.push(b);
  });
  crumbs.replaceChildren(...parts);
}

function renderPicked() {
  const items = [...send.picked.values()];
  $("picked-wrap").hidden = items.length === 0;
  $("picked").replaceChildren(...items.map((p) => el("li", "", `${p.root}/${p.path}`)));
  $("send-go").disabled = items.length === 0;
  $("send-hint").textContent = items.length === 0 ? "Choose files or folders above." : count(items.length, "item", "items") + " chosen.";
}

async function startSend() {
  const btn = $("send-go");
  btn.disabled = true;
  $("send-hint").textContent = "Preparing the files";
  try {
    const res = await call("/send", { paths: [...send.picked.values()] });
    showTicket(res);
    send.picked.clear();
    renderPicked();
  } catch (e) {
    $("send-hint").textContent = e.message;
    btn.disabled = false;
  }
}

function showTicket(res) {
  $("send-pick").hidden = true;
  $("ticket").hidden = false;
  $("ticket-value").textContent = res.ticket.text;
  $("ticket-qr").src = "data:image/svg+xml;base64," + btoa(res.qr_svg);
  addJob({ id: res.job_id, kind: "send", state: "running", bytes_total: res.bytes, bytes_done: 0, files: res.files });
}

async function copyTicket() {
  const btn = $("ticket-copy");
  try {
    await navigator.clipboard.writeText($("ticket-value").textContent);
    btn.textContent = "Copied";
  } catch {
    getSelection().selectAllChildren($("ticket-value"));
    btn.textContent = "Press Ctrl+C";
  }
  setTimeout(() => { btn.textContent = "Copy ticket"; }, 2000);
}

function closeTicket() {
  $("ticket").hidden = true;
  $("ticket-value").textContent = "";
  $("ticket-qr").removeAttribute("src");
  $("send-pick").hidden = false;
  openFolder(send.root, send.path);
}

// ---- receive ------------------------------------------------------------------------------------

let offerTicket = null;

function recvError(msg) {
  const p = $("recv-error");
  p.hidden = !msg;
  p.textContent = msg || "";
}

async function inspect(ev) {
  ev.preventDefault();
  recvError("");
  $("offer").hidden = true;
  const text = $("recv-ticket").value.trim();
  if (!text) { recvError("Paste a ticket first."); return; }
  try {
    const info = await call("/inspect", { ticket: { text } });
    offerTicket = text;
    const parts = [count(info.files, "file", "files")];
    if (Number(info.folders)) parts.push(count(info.folders, "folder", "folders"));
    $("offer-size").textContent = `${parts.join(" in ")}, ${bytes(info.bytes)}`;
    const names = (info.names || []).map((n) => el("li", "", n.text));
    if (Number(info.more_names)) names.push(el("li", "", `and ${info.more_names} more`));
    $("offer-names").replaceChildren(...names);
    $("offer").hidden = false;
    $("recv-go").focus();
  } catch (e) {
    recvError(e.message);
  }
}

async function startReceive() {
  recvError("");
  const btn = $("recv-go");
  btn.disabled = true;
  try {
    const root = $("recv-root").value;
    const res = await call("/receive", { ticket: { text: offerTicket }, out: { root, path: "" } });
    addJob({ id: res.job_id, kind: "receive", state: "running", bytes_total: 0, bytes_done: 0, into: root });
    offerTicket = null;
    $("recv-ticket").value = "";
    $("offer").hidden = true;
  } catch (e) {
    recvError(e.message);
  } finally {
    btn.disabled = false;
  }
}

// ---- transfers ----------------------------------------------------------------------------------

const jobs = new Map();

function addJob(job) {
  jobs.set(job.id, { ...jobs.get(job.id), ...job });
  renderJobs();
}

function jobTitle(j) {
  if (j.kind === "send") return j.files ? `Sending ${count(j.files, "file", "files")}` : "Sending";
  return j.into ? `Receiving into ${j.into}` : "Receiving";
}

function jobMeta(j) {
  const bits = [];
  if (j.bytes_total) bits.push(`${bytes(j.bytes_done)} of ${bytes(j.bytes_total)}`);
  if (j.rate && j.state === "running") bits.push(`${bytes(j.rate)}/s`);
  if (j.path) bits.push(j.path === "direct" ? "direct" : "through a relay");
  if (j.kind === "send" && j.receivers) bits.push(count(j.receivers, "receiver", "receivers"));
  if (j.warning) bits.push(j.warning);
  return bits.join(", ");
}

const STATE_TEXT = { queued: "Waiting", running: "Moving", done: "Done", failed: "Failed", cancelled: "Cancelled" };

function renderJobs() {
  const list = $("transfers");
  $("transfers-empty").hidden = jobs.size > 0;
  const rows = [...jobs.values()].reverse().map((j) => {
    const li = el("li", "xfer");
    li.dataset.state = j.state;
    li.append(el("span", "xfer-title", jobTitle(j)));
    const side = el("div", "xfer-side");
    side.append(el("span", "xfer-state", STATE_TEXT[j.state] || j.state));
    if (j.state === "running" || j.state === "queued") {
      const stop = el("button", "quiet", "Cancel");
      stop.type = "button";
      stop.addEventListener("click", () => call(`/jobs/${encodeURIComponent(j.id)}/cancel`, {}).catch(() => {}));
      side.append(stop);
    }
    li.append(side, el("span", "xfer-meta", jobMeta(j)));
    const ch = el("div", "channel");
    const fill = el("span");
    const pct = j.state === "done" ? 100 : j.bytes_total ? (100 * Number(j.bytes_done)) / Number(j.bytes_total) : 0;
    fill.style.width = `${Math.min(100, pct)}%`;
    ch.append(fill);
    li.append(ch);
    return li;
  });
  list.replaceChildren(...rows);
}

function onEvent(ev) {
  const j = jobs.get(ev.job_id) || { id: ev.job_id, kind: "receive", state: "running", bytes_done: 0, bytes_total: 0 };
  if (ev.dropped) { loadJobs().catch(() => {}); return; }
  if (ev.started) { j.kind = ev.started.kind; j.bytes_total = ev.started.bytes; j.files = ev.started.files; }
  if (ev.connected) j.path = ev.connected.path;
  if (ev.path_changed) j.path = ev.path_changed.path;
  if (ev.progress) { j.bytes_done = ev.progress.bytes_done; j.bytes_total = ev.progress.bytes_total; j.rate = ev.progress.bytes_per_second; }
  if (ev.warning) j.warning = ev.warning.message;
  if (ev.done) j.state = ev.done.state || "done";
  jobs.set(j.id, j);
  renderJobs();
}

async function loadJobs() {
  const { jobs: list } = await call("/jobs");
  for (const j of list) jobs.set(j.id, { ...jobs.get(j.id), ...j });
  renderJobs();
}

// ---- theme --------------------------------------------------------------------------------------

function initTheme() {
  let saved = null;
  try { saved = localStorage.getItem("sluice-theme"); } catch { /* ignore */ }
  if (saved) document.documentElement.dataset.theme = saved;
  $("theme").addEventListener("click", () => {
    const dark = getComputedStyle(document.documentElement).colorScheme.includes("dark") &&
      document.documentElement.dataset.theme !== "light";
    const next = dark ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem("sluice-theme", next); } catch { /* ignore */ }
  });
}

// ---- start --------------------------------------------------------------------------------------

async function start() {
  initTheme();
  const status = $("daemon");
  if (!KEY && !window.SLUICE_MOCK) {
    status.textContent = "Not connected";
    status.dataset.state = "down";
    $("locked").hidden = false;
    return;
  }
  let label = "";
  $("panes").hidden = false;
  $("transfers-wrap").hidden = false;
  $("send-go").addEventListener("click", startSend);
  $("ticket-copy").addEventListener("click", copyTicket);
  $("ticket-done").addEventListener("click", closeTicket);
  $("recv-form").addEventListener("submit", inspect);
  $("recv-go").addEventListener("click", startReceive);
  try {
    const v = await call("/version");
    status.textContent = `Sluice ${v.version}`;
    status.dataset.state = "up";
    label = status.textContent;
    await Promise.all([loadRoots(), loadJobs()]);
  } catch (e) {
    status.textContent = e.message;
    status.dataset.state = "down";
    return;
  }
  watchEvents(onEvent, (up) => {
    status.dataset.state = up ? "up" : "down";
    status.textContent = up ? label : "Lost the daemon, trying again";
  }, () => {
    // Events missed while the stream was down are read from the jobs.
    loadJobs().catch(() => {});
  });
}

start();
