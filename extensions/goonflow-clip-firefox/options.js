// Options page: stores port + token in browser.storage.local and offers a
// handshake test against the local companion API.
//
// #38 also shows what the app holds for the three login sites. This page only
// ever reads that state back (`GET /api/cookie?site=…`, which never returns the
// cookie value) and asks the app to re-probe a cookie it already holds (`POST`
// with an empty cookie field). It never reads the browser's cookies and never
// sends one.
//
// Firefox variant: no Firefox-specific handling is needed here — storage and
// fetch behave the same, and the cookies API is only used in the popup, where
// this add-on talks through `browser.*` (see popup.js).

const $ = (id) => document.getElementById(id);

async function load() {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  $("port").value = port || "";
  $("token").value = token || "";
}

function status(msg, ok) {
  const el = $("status");
  el.textContent = msg;
  el.className = ok ? "ok" : "bad";
}

$("save").addEventListener("click", async () => {
  const port = Number($("port").value) || 0;
  const token = $("token").value.trim();
  await chrome.storage.local.set({ port, token });
  status(port && token ? "Saved." : "Saved — but port and token are both required to clip.", !!port && !!token);
  refreshCookies();
});

$("test").addEventListener("click", async () => {
  const port = Number($("port").value) || 0;
  if (!port) return status("Enter the port first.", false);
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/handshake`);
    const v = await res.json();
    if (v.app === "TheGoonFlow") {
      status(`Connected to The Goon Flow ${v.version}${v.locked ? " (vault locked — unlock it)" : ""}.`, true);
    } else {
      status("Something else answered on that port.", false);
    }
  } catch {
    status("No answer — is the desktop app running?", false);
  }
});

// ---------- sign-in cookies (#38) ----------

// Site ids exactly as the app names them (its KNOWN_SITES), which is what the
// vault keys its per-site cookie by. Note e-hentai.org is "ehentai": the host
// has a hyphen, the site id does not.
const LOGIN_SITES = [
  { site: "f95", host: "f95zone.to" },
  { site: "ehentai", host: "e-hentai.org" },
  { site: "exhentai", host: "exhentai.org" },
];

function showState(site, cls, text) {
  const el = $(`state-${site}`);
  if (!el) return;
  el.textContent = text;
  el.className = `cookie-state ${cls}`;
}

// Reads the app's own record. The response has set/ok/checkedAt and never the
// cookie itself, so there is nothing here that could put one on screen.
async function readState(site) {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  if (!port || !token) return showState(site, "", "not paired - paste the port + token above");
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/cookie?site=${encodeURIComponent(site)}`, {
      headers: { authorization: `Bearer ${token}` },
    });
    const v = await res.json().catch(() => ({}));
    if (res.status === 401) return showState(site, "bad", "not paired - the app did not accept this token");
    if (res.status === 423) return showState(site, "bad", "vault locked - unlock the app to see this");
    if (res.status !== 200) return showState(site, "bad", `the app refused it (http ${res.status})`);
    if (!v.set) return showState(site, "", `no cookie stored for ${site}`);
    // checkedAt is an epoch-seconds string, not a date string
    const when = v.checkedAt ? new Date(Number(v.checkedAt) * 1000).toLocaleString() : null;
    if (v.ok === true) return showState(site, "ok", `cookie stored - the app's last probe passed (${when})`);
    if (v.ok === false) return showState(site, "bad", `cookie stored - the app's last probe failed (${when})`);
    return showState(site, "", "cookie stored - the app has not probed it yet");
  } catch {
    return showState(site, "bad", `nothing on port ${port} - is the desktop app running?`);
  }
}

// The Check button: ask the app to probe the cookie it already holds (one page
// fetch, on the site's polite delay). An empty cookie field is the signal for
// "re-check, do not store", so no cookie ever leaves this page in that call.
async function recheck(site) {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  if (!port || !token) return showState(site, "", "not paired - paste the port + token above");
  showState(site, "", `asking the site once for ${site}...`);
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/cookie`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ site, cookie: "" }),
    });
    const v = await res.json().catch(() => ({}));
    if (res.status === 401) return showState(site, "bad", "not paired - the app did not accept this token");
    if (res.status === 423) return showState(site, "bad", "vault locked - unlock the app to check this");
    const note = (v.verdict && v.verdict.note) || v.error || `the app refused it (http ${res.status})`;
    return showState(site, v.ok ? "ok" : "bad", note);
  } catch {
    return showState(site, "bad", `nothing on port ${port} - is the desktop app running?`);
  }
}

function refreshCookies() {
  return Promise.all(LOGIN_SITES.map((s) => readState(s.site)));
}

for (const btn of document.querySelectorAll("button[data-check]")) {
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    try {
      await recheck(btn.dataset.check);
    } finally {
      btn.disabled = false;
    }
  });
}

load();
refreshCookies();
