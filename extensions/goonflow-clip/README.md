# GoonFlow Clip — browser extension (M4, issue #19)

An MV3 extension that sends a link or image from any Chrome/Edge/Firefox page
into your GoonFlow vault — the *same* ingestion path as the app's `+ Clip URL`
button, over the local companion API.

## What it does
- Click the toolbar icon → status panel: pairing state, app reachable?, **Clip
  this page** button.
- Right-click a link, image, page, or selected URL → **Clip … to GoonFlow**.
- The desktop app classifies it (booru / e-hentai / DLsite / f95 / direct media /
  `file:///`), fetches metadata politely, and stores it in the encrypted vault.
- A desktop notification confirms the clip (or tells you what went wrong).

## Install (Chrome / Edge — unpacked)
1. Open `chrome://extensions` (or `edge://extensions`).
2. Turn on **Developer mode** (top right).
3. **Load unpacked** → select this `extensions/goonflow-clip` folder.

## Install (Firefox)
Use the sibling folder **`extensions/goonflow-clip-firefox`** — same extension,
but its manifest uses a classic background page (`background.scripts`) because
Firefox's MV3 service-worker support is gated behind browser settings (and
hardened/privacy-tuned Firefox profiles disable service workers entirely; this
variant needs no service workers at all).

**Temporary (any Firefox 109+):**
1. Open `about:debugging#/runtime/this-firefox`.
2. Click **Load Temporary Add-on…**
3. Firefox asks for a *file*: pick the `manifest.json` *inside*
   `extensions/goonflow-clip-firefox`.

The extension loads instantly, but **temporary add-ons are removed when Firefox
closes** — reload it this way after each restart, or make it permanent:

**Permanent (Developer Edition / Nightly only):**
1. In `about:config` set `xpinstall.signatures.required` to `false`.
2. Zip the *contents* of `extensions/goonflow-clip-firefox` (manifest.json at the zip
   root) into e.g. `goonflow-clip.zip` (rename `.xpi` if you like).
3. `about:addons` → gear icon → **Install Add-on From File…** → pick the zip.
Stable Firefox refuses unsigned add-ons; the other route is uploading it to
addons.mozilla.org as an unlisted (public, but not listed) signing — it contains
no code that touches the internet, review is usually fast.

## Pair (once per app launch)
The app binds a **random local port** and mints a **fresh token every start**.
1. In GoonFlow, open **Sources → Browser pairing**.
2. Copy the **port** and **token**.
3. In the extension, right-click its icon → **Manage extension → Extension
   options** (or the details page → Extensions options) and paste both, then
   **Test connection**.

Because the token rotates on every launch, re-paste it after restarting the app.

## Privacy
- The extension's only host permissions are `http://127.0.0.1/*` and
  `http://localhost/*`. It cannot reach the internet; only the local app can,
  and only the app talks to the content sites.
- The token gates writes: an unrelated local process that can't read the token
  can't inject clips.

## Files
- `manifest.json` — MV3 manifest, context-menu + storage + notifications.
- `background.js` — service worker: menus + the clip request.
- `options.html` / `options.js` — paste port/token, test the handshake.
- `icon48.png` — toolbar/notification icon.
