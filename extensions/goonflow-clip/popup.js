// GoonFlow Clip popup — pairing status + one-click clip of the current page.
// Plain JS: the popup runs only while it is open.

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

function say(el, cls, text) {
  el.className = `status ${cls}`;
  el.textContent = text;
}

init();

async function init() {
  const status = document.getElementById("status");
  const clipBtn = document.getElementById("clip");
  const urlEl = document.getElementById("url");

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

  if (!port || !token) {
    say(status, "bad", "Not paired yet: open the options page and paste the port + token from the app's Sources panel.");
    clipBtn.disabled = true;
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
    }
  } catch {
    say(status, "bad", `Nothing on port ${port} — is the desktop app running? Its port changes every launch; re-copy it under Sources → Browser pairing.`);
    clipBtn.disabled = true;
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
}
