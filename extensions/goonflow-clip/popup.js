// GoonFlow Clip popup — pairing status + one-click clip of the current page.
// Plain JS: the popup runs only while it is open.
//
// #38 adds the other half of the hand-off: on the three sites whose pages are a
// login wall, the popup can read *this browser's* sign-in cookies (the cookies
// API sees HttpOnly cookies, which is exactly what document.cookie and a
// hand-paste from DevTools trip over) and hand them to the app over the local
// companion API. Nothing happens by itself: the button exists, the user clicks
// it. The cookie string is never written to chrome.storage and never sent
// anywhere except http://127.0.0.1 (DESIGN D20) — the vault holds it, this
// extension is a pipe.

async function settings() {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  return { port: Number(port) || 0, token: token || "" };
}

async function activeTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  return tab;
}

async function clip(url) {
  const { port, token } = await settings();
  const res = await fetch(`http://127.0.0.1:${port}/api/clip`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ url }),
  });
  return { status: res.status, body: await res.json().catch(() => ({})) };
}

// host -> the site id the app knows (its KNOWN_SITES names, which are what the
// vault's per-site cookie column is keyed by: e-hentai.org is "ehentai", no
// hyphen, while the host has one). Only these hosts get the cookie button.
const LOGIN_SITES = {
  "f95zone.to": { site: "f95", needs: "xf_session and xf_user" },
  "e-hentai.org": { site: "ehentai", needs: "ipb_member_id and ipb_pass_hash" },
  "exhentai.org": { site: "exhentai", needs: "ipb_member_id and ipb_pass_hash" },
};

function loginSite(url) {
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
  if (host.startsWith("www.")) host = host.slice(4);
  const hit = LOGIN_SITES[host];
  return hit ? { host, ...hit } : null;
}

// One header line in exactly the order the browser handed it over: no
// filtering, no sorting, no de-duping. Some sites pair a csrf token with the
// session and some are picky about order, so a "tidied" cookie is a broken
// cookie. The value lives in a local variable only — not chrome.storage, not
// the page, and the one place it is ever sent is 127.0.0.1.
// Querying by `domain` rather than `url` also pulls in cookies scoped to
// subdomains (attachments.f95zone.to); the browser would not have sent those to
// the site either, but a cookie a site does not know is inert, and dropping
// "foreign" entries is exactly the filtering that breaks csrf/session pairs.
async function readCookieHeader(domain) {
  const jar = await chrome.cookies.getAll({ domain });
  return { count: jar.length, header: jar.map((c) => `${c.name}=${c.value}`).join("; ") };
}

async function sendCookie(port, token, site, cookie) {
  const res = await fetch(`http://127.0.0.1:${port}/api/cookie`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ site, cookie }),
  });
  return { status: res.status, body: await res.json().catch(() => ({})) };
}

// What the app's answer means, in words. The app's own `note` and the names of
// the failed `checks` are shown as-is (the app never echoes the cookie back);
// the status code carries the rest: 401 = not paired, 423 = vault locked,
// 400 = it refused the site or an empty cookie, 422 = the paste was refused and
// nothing was stored.
function verdict(code, v, login) {
  if (code === 401)
    return ["bad", "Not paired: the app did not accept the token. It is fresh every time the app starts - re-copy port + token under Sources -> Browser pairing."];
  if (code === 423)
    return ["bad", "The vault is locked, so the app could not store it. Unlock the app, then click again."];
  if (code === 400)
    return ["bad", `The app refused it: ${v.error || "unknown site or empty cookie"} (sent site id "${login.site}").`];
  if (code === 422) {
    // the paste reached the app and was refused before it was stored, so the
    // app's own note is the useful part: it names the key that was missing.
    const w = v.verdict || {};
    const failed = (w.checks || []).filter((c) => c && !c.ok).map((c) => c.name).filter(Boolean);
    return [
      "bad",
      `Nothing was stored: ${w.note || "the app says that is not a session cookie"}${
        failed.length ? ` (checks that did not pass: ${failed.join(", ")})` : ""
      }. It wants ${login.needs}.`,
    ];
  }
  if (code === 200 && v.ok)
    return ["ok", `Saved for ${v.site || login.site}: ${v.note || "the app accepted it."}`];
  if (code === 200) {
    const failed = (v.checks || []).filter((c) => c && !c.ok).map((c) => c.name).filter(Boolean);
    return [
      "bad",
      `Stored for ${v.site || login.site}, but the app says it is not a working login${
        failed.length ? ` (checks that did not pass: ${failed.join(", ")})` : ""
      }. ${v.note || `It wants ${login.needs}.`}`,
    ];
  }
  return ["bad", v.error ? `The app said: ${v.error}` : `The app refused it (http ${code}).`];
}

function say(el, cls, text) {
  el.hidden = false;
  el.className = `status ${cls}`;
  el.textContent = text;
}

init();

async function init() {
  const status = document.getElementById("status");
  const clipBtn = document.getElementById("clip");
  const urlEl = document.getElementById("url");
  const cookieRow = document.getElementById("cookie-row");
  const cookieBtn = document.getElementById("cookie");
  const cookieNeeds = document.getElementById("cookie-needs");
  const cookieStatus = document.getElementById("cookie-status");

  document.getElementById("options").onclick = () => chrome.runtime.openOptionsPage();

  const { port, token } = await settings();
  let tab = null;
  try {
    tab = await activeTab();
    if (tab?.url) urlEl.textContent = tab.url;
    urlEl.title = tab?.url || "";
  } catch {
    // no active tab access — clip button stays disabled below
  }

  // The cookie button only exists on sites that need a login, and only this
  // page's host decides that. No site-wide cookie access is ever hinted at.
  const login = tab?.url ? loginSite(tab.url) : null;
  if (login) {
    cookieRow.hidden = false;
    say(cookieNeeds, "muted", `How to tell the cookie is real: a ${login.site} cookie must contain ${login.needs}.`);
  }

  if (!port || !token) {
    say(status, "bad", "Not paired yet: open the options page and paste the port + token from the app's Sources panel.");
    clipBtn.disabled = true;
    cookieBtn.disabled = true;
    if (login) say(cookieStatus, "muted", "Pair the extension first: paste the app's port + token on the options page.");
    return;
  }

  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/handshake`);
    const v = await res.json().catch(() => ({}));
    if (res.ok && v.app === "TheGoonFlow") {
      say(status, "ok", v.locked ? "App reachable — but the vault is LOCKED, unlock it to clip." : "Paired with the app. Ready.");
      clipBtn.disabled = !tab?.url;
    } else {
      say(status, "bad", `Port ${port} answered, but not as GoonFlow (stale pairing? re-copy port + token).`);
      clipBtn.disabled = true;
      cookieBtn.disabled = true;
    }
  } catch {
    say(status, "bad", `Nothing on port ${port} — is the desktop app running? Its port changes every launch; re-copy it under Sources → Browser pairing.`);
    clipBtn.disabled = true;
    cookieBtn.disabled = true;
  }

  clipBtn.onclick = async () => {
    if (!tab?.url) return;
    clipBtn.disabled = true;
    say(status, "muted", "clipping…");
    try {
      const { status: code, body: v } = await clip(tab.url);
      if (code === 200 && v.ok) {
        const r = v.report || {};
        say(status, "ok", r.created ? `Clipped: ${r.site}${r.title ? ` — ${r.title}` : ""}` : `Already in your flow (${r.site}).`);
      } else {
        say(status, "bad", v.error || `the app refused (http ${code})`);
      }
    } catch (e) {
      say(status, "bad", "app not answering — is it running?");
    }
    clipBtn.disabled = false;
  };

  cookieBtn.onclick = async () => {
    if (!login) return;
    cookieBtn.disabled = true;
    let jar;
    try {
      jar = await readCookieHeader(login.host);
    } catch {
      say(cookieStatus, "bad", `The browser would not give up its cookies for ${login.host} - the extension needs the cookies permission (reload it after changing the manifest).`);
      cookieBtn.disabled = false;
      return;
    }
    if (!jar.header) {
      say(cookieStatus, "bad", `This browser holds no cookies for ${login.host} - a cookie copied while you were logged out is worthless, so nothing was sent.`);
      cookieBtn.disabled = false;
      return;
    }
    say(cookieStatus, "muted", `sending ${jar.count} cookie(s) for ${login.site} to the app on port ${port}...`);
    try {
      const { status: code, body: v } = await sendCookie(port, token, login.site, jar.header);
      const [cls, text] = verdict(code, v, login);
      say(cookieStatus, cls, `${jar.count} cookie(s) from ${login.host}. ${text}`);
    } catch {
      say(cookieStatus, "bad", `Nothing on port ${port} - is the desktop app running? Its port changes every launch; re-copy it under Sources -> Browser pairing.`);
    }
    cookieBtn.disabled = false;
  };
}
