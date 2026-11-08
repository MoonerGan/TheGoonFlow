// GoonFlow Clip — MV3 background worker (#19).
//
// One job: right-click → send a URL to the local GoonFlow companion API
// (http://127.0.0.1:<port>/api/clip, bearer token from the app's Sources
// panel). No analytics, no remote calls — host_permissions are localhost
// only, and the extension works while the desktop app is running.

const MENUS = [
  { id: "gf-clip-page", title: "Clip this page to GoonFlow", contexts: ["page", "editable"] },
  { id: "gf-clip-link", title: "Clip link to GoonFlow", contexts: ["link"] },
  { id: "gf-clip-image", title: "Clip image to GoonFlow", contexts: ["image"] },
  { id: "gf-clip-selection", title: "Clip selected URL to GoonFlow", contexts: ["selection"] },
];

chrome.runtime.onInstalled.addListener(() => {
  for (const m of MENUS) chrome.contextMenus.create(m);
});

async function settings() {
  const { port, token } = await chrome.storage.local.get(["port", "token"]);
  return { port: Number(port) || 0, token: token || "" };
}

function targetUrl(info, tab) {
  const sel = (info.selectionText || "").trim();
  return (
    info.srcUrl || // image context
    info.linkUrl || // link context
    (isUrl(sel) ? sel : "") || // selection that is itself a URL
    tab?.url || // page context
    ""
  );
}

function isUrl(s) {
  return /^https?:\/\//i.test(s) || /^file:\/\/\//i.test(s);
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = targetUrl(info, tab);
  if (!url) return notify("GoonFlow", "Nothing clippable there.");
  const { port, token } = await settings();
  if (!port || !token) {
    chrome.runtime.openOptionsPage();
    return notify("GoonFlow", "Pair me first: open extension options and paste the app's port + token.");
  }
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/clip`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ url }),
    });
    const v = await res.json().catch(() => ({}));
    if (res.ok && v.ok) {
      const r = v.report || {};
      notify(
        r.created ? "Clipped to GoonFlow" : "Already in your flow",
        `${r.site || "?"}${r.title ? ` — ${r.title}` : ""}`,
      );
    } else {
      notify("GoonFlow could not clip", v.error || `http ${res.status} — is the vault unlocked?`);
    }
  } catch (e) {
    notify("GoonFlow is not answering", "Start the desktop app (the companion port changes each launch — re-copy it under Sources).");
  }
});

function notify(title, message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: "icon48.png",
    title,
    message,
  });
}
