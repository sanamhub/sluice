"use strict";
// How the page talks to the daemon: the key it was given, one call at a time, and the stream of
// events. It loads before app.js and shares its scope.

const API = "/api/v1";

// ---- key ----------------------------------------------------------------------------------------

function takeKey() {
  const frag = new URLSearchParams(location.hash.slice(1));
  const fromFrag = frag.get("key");
  if (fromFrag) {
    try { sessionStorage.setItem("sluice-key", fromFrag); } catch { /* storage blocked: keep in memory */ }
    history.replaceState(null, "", location.pathname + location.search);
    return fromFrag;
  }
  try { return sessionStorage.getItem("sluice-key"); } catch { return null; }
}
const KEY = takeKey();

// ---- transport ----------------------------------------------------------------------------------

class ApiError extends Error {}

async function call(path, body) {
  if (window.SLUICE_MOCK) return window.SLUICE_MOCK.call(path, body);
  const init = { method: body === undefined ? "GET" : "POST", headers: { Authorization: `Bearer ${KEY}` } };
  if (body !== undefined) {
    init.headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(API + path, init);
  } catch {
    throw new ApiError("The daemon is not answering. Start it with `sluice daemon --ui`.");
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new ApiError(data.error || `The daemon refused the request (${res.status}).`);
  return data;
}

// Events arrive as one JSON object per line. EventSource cannot send a header, so this reads the
// stream with fetch.
async function watchEvents(onEvent, onState, onConnected) {
  if (window.SLUICE_MOCK) return window.SLUICE_MOCK.watch(onEvent, onState);
  let wait = 500;
  for (;;) {
    try {
      const res = await fetch(`${API}/events`, { headers: { Authorization: `Bearer ${KEY}` } });
      if (!res.ok || !res.body) throw new Error();
      onState(true);
      onConnected();
      wait = 500;
      const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
      let buf = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += value;
        let nl;
        while ((nl = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, nl).trim();
          buf = buf.slice(nl + 1);
          if (line) onEvent(JSON.parse(line));
        }
      }
    } catch { /* fall through to retry */ }
    onState(false);
    await new Promise((r) => setTimeout(r, wait));
    wait = Math.min(wait * 2, 8000);
  }
}
