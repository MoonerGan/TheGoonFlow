// Options page: stores port + token in chrome.storage.local and offers a
// handshake test against the local companion API.

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

load();
