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
- On **f95zone.to**, **e-hentai.org** and **exhentai.org** the popup also shows
  **Send this page's sign-in cookie** (#38) — see *Sign-in cookies* below. It
  only appears on those three hosts and only ever fires on your click.

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

## Sign-in cookies (#38)

f95, E-hentai and Exhentai are a login wall: without a session cookie an f95
thread is a "register to view" page and E-hentai hides half its gallery list. The
app keeps that cookie in the vault (same box as *Sources → cookie*, same
`Cookie:` header on every fetch). Today you paste it by hand out of DevTools;
this flow replaces the ritual:

1. Open the site in this browser and log in there.
2. Click the toolbar icon → **Send this page's sign-in cookie**.
3. Read the verdict under the button.

- The button exists only when the open tab's host is `f95zone.to`,
  `e-hentai.org` or `exhentai.org`. Nothing is read or sent on any other host,
  and nothing happens on its own: no page load, no startup, no retry loop.
- It reads *this browser's* jar for that one host through the cookies API, which
  sees `HttpOnly` cookies (`xf_session`, `ipb_pass_hash`) that a DevTools-free
  paste usually misses. The jar is posted verbatim as one `k=v; k2=v2` line in
  the order the browser gave it — no filtering, no sorting: sites pair a csrf
  token with the session and some are picky about order, so a tidied cookie is a
  broken cookie.
- The app answers with what it detected, never with the cookie: a one-line `note`
  plus the named checks it ran (on f95 `xf_session`, with Cloudflare's
  `cf_clearance` as a warning; `ipb_member_id` + `ipb_pass_hash` on E-hentai and
  Exhentai). A green line means the vault holds a login the site also
  recognised. A red line names the checks that did not pass, or says the site
  answered as a guest - usually that you were logged out in this browser when you
  clicked, or that the session has since gone stale.
- *Not paired* (http 401) means the token rotated: re-copy port + token under
  *Sources → Browser pairing*. *Vault locked* (http 423) means unlock the app and
  click again. *Nothing was stored* (http 422) means the app refused the paste
  before saving it, and its note names the key that was missing.
- The options page lists all three sites with what the app recorded (`set`,
  whether its own probe passed, and the last check time). **Check** asks the app
  to probe the cookie it already holds — one page fetch on that site's polite
  delay — and prints the app's own verdict line. It sends no cookie: an empty
  `cookie` field means "re-check what the vault holds". Neither page can display
  one.

Site ids are the app's own (`f95`, `ehentai`, `exhentai`) — note `ehentai` has no
hyphen even though the host `e-hentai.org` does.

## Privacy

- Every request this extension makes is a `fetch` to `http://127.0.0.1:<port>` —
  clip, handshake and cookie alike. There is no other outbound call anywhere in
  the code, no analytics, no remote host, and `chrome.storage` / `browser.storage`
  only ever holds the pairing port and token. **The cookie is never stored by the
  extension**: it is read, handed to the local app in one POST, and dropped. The
  vault holds it (DESIGN D20); the extension is a pipe.
- The token gates writes: an unrelated local process that can't read the token
  can't inject clips — and can't take a cookie either, because the extension only
  talks to a port it was told about.
- **Read this before installing: the manifest now asks for the `cookies`
  permission plus host access to three sites.** Chrome/Edge say it out loud when
  you load the extension: *"Read your browsing history"* (that is what the
  `cookies` permission is called in the warning) and *"Read and change all your
  data on the websites you visit on: f95zone.to, e-hentai.org, exhentai.org"*.
  Firefox lists the same two things in its add-on permission prompt. That is a
  real trust question, not a formality: with those grants the extension can read
  your **login cookies** (including `HttpOnly` ones) on those three hosts, which
  is exactly what a session hijacker would want.
  - It is the minimum that works: the cookies API refuses to answer for a host the
    extension has no host permission for, so a cookie hand-off without those three
    entries is impossible. They are declared for **reading the local jar only** —
    no code in this extension issues a request to those hosts, and content scripts
    are not used at all, so nothing runs inside those pages.
  - If you would rather not grant it, delete the `cookies` permission and the three
    `https://…` entries from `host_permissions`; clipping keeps working, and the
    popup's cookie path fails closed with a plain "the browser would not give up
    its cookies" line.
- Unpacked / temporary installs are the only distribution today, so the grant is
  visible in `chrome://extensions` → *Site access* at any time, and revoking it
  (or removing the extension) takes the capability away immediately.

## Chrome vs Firefox

Both folders ship the same behaviour and the same permissions; the differences
that are actually real:

- `background`: Chrome uses a `service_worker`, Firefox a classic `scripts`
  background page (Firefox MV3 service workers are gated/off in hardened profiles).
- The Firefox build's new cookie code calls `browser.cookies` (`api.cookies`);
  `chrome.*` in Firefox is only a compatibility alias. The option pages need no
  such handling — storage and `fetch` are the same there.
- Neither build uses the `action` API, so the one genuine Chrome/Firefox split
  (`chrome.action` vs `browser.browserAction`) never comes up; the manifest's
  `action` key is accepted by both.
- Firefox reads the **default cookie store**. A login that lives in a Container
  tab or in a Private window is not picked up (no `cookieStoreId` is passed: the
  vault holds one cookie per site, so the default jar is the honest answer).
  Chrome has no cookie containers, so this is a Firefox-only limitation, and it
  shows up as the "no cookies for this host" line rather than a wrong cookie.
- `strict_min_version` differs (121 vs 109) because only the Chrome build needs
  MV3 service workers.

## Files
- `manifest.json` — MV3 manifest, context-menu + storage + notifications +
  cookies (both variants declare `cookies` and the three login hosts).
- `background.js` — service worker / background page: menus + the clip request.
- `popup.html` / `popup.js` — pairing state, clip button, and the
  **Send this page's sign-in cookie** button + verdict on the three login sites.
- `options.html` / `options.js` — paste port/token, test the handshake, per-site
  cookie state read back from the app.
- `icon48.png` — toolbar/notification icon.
