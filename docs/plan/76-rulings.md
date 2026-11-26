## Addendum after review (2026-11-25) — rulings that change the plan

**All twelve recommendations in §9 are approved** as written, with these three amendments from review.

### A. "Unknowable" was the wrong word for animation — there are three real paths

The plan said animated-ness "is genuinely unknowable today" for anything but gif. That was a statement about *today's code*, not about the files, and it read like a shrug. It is fixable, and there is a cheap fallback the plan did not have:

1. **A `file-extension` tag type** (new, additive): every item whose source names a file gets its extension as a tag of its own type — `mp4`, `gif`, `webp`, `png`… Cheap, always available, needs no decode, and gives the grid a filterable vocabulary even before any probing exists. `type:` in the grammar keeps its meaning (what GoonFlow computed); `ext:` becomes the "what the file calls itself" side, and the two are allowed to disagree honestly (a `.jpg` link that serves AVIF is `ext:jpg` **and** `image/avif`).
2. **Animation read from the file's own metadata**, for stored bytes: gif (by mime), **APNG** (PNG `acTL` chunk), **animated WebP** (RIFF `ANIM`/`ANMF` chunks) are all readable by scanning the container's chunk table without decoding a frame — well under a kilobyte of code each. That makes `animated=1` a *fact from the file*, not a guess, for the three formats a media library actually holds.
3. **Animated MP4/WebP variants** (a still `image/mp4` that is really a motion picture, motion posters in `mp4`) stay NULL until the video-probe path exists, and the note says "not probed" rather than "not animated".

Sub-issue: **media metadata: extension tag type + animation read from container chunks**.

### B. Duration and size: engineer the real path, no half-measures

The plan proposed "unknown duration never matches, we'll probe later". Rejected as too partial. The rule now: **every media item gets a size, and every video gets a duration, from one of three engineered sources** —

| item shape | size | duration |
|---|---|---|
| stored in the vault | `media.bytes` (already have it; add the column) | probe the bytes with the OS decoder (Media Foundation `IMFSourceReader` / Shell property store), which also yields **width and height of the original** — fixing review note 1 in the same stroke |
| on disk (`file://` source) | `stat`, cached | same OS probe on the path |
| kept as a link | **one ranged request**: `GET … Range: bytes=0-0` (or HEAD) → `Content-Length`, without pulling the file | fetch the **prefix** the ISO-BMFF/Matroska header needs (`moov` before `mdat`, or a WebM/MKV EBML head) and read the duration from it. Measured on the reported rule34 clip: `Range: bytes=0-65535` → `206`, and its `moov` sat at byte 32 of a 59 MB file — the whole duration/size/dimensions fact set is available from the **first ~225 KB** of that file, and booru video files are laid out that way |
| the site API offers it | booru `file.duration` / `file.size` fields, which ingest **currently throws away** | import them; they are free |

Plus the policy hook you named: a **"always store videos" setting** (lives in the settings tab, #75) makes the vault-probe path the default, so the fact arrives for free as part of a download we were doing anyway. Where a host refuses ranges (rule34's images host sends no `Accept-Ranges` but still answers `206`), the probe **tests** range support instead of assuming it from a header.

Sub-issue: **media facts engine: size + duration + original dimensions for vault / disk / link items**.

### C. Filtering and bulk actions apply to the whole set, never to the loaded page

Explicit requirement from review: *"As long as the filter applies to the full set still. Not just paged data."* So:

- the query is evaluated **server-side over the whole library**; the page (first 300 + load more) is a display detail;
- the count line is `N matches · M total` where `N` is the full match count, not the page length;
- **"Select all" selects the whole query result**, and the bulk actions (#73 *Store selected*, *Re-fetch metadata*, *Delete selected*) act on the full match set, with a server-side ids/count endpoint behind them. Today "Select all" can only reach the 300 loaded rows (`db.rs:425-427`) — that limitation is in scope for this card, not deferred to #73.

---

## Scope note: blockers are inside this card

Anything that blocks the booru-like tag/search feature is **in scope for #76 itself**; genuinely complex pieces get sub-issues rather than being left as a sentence in a comment. That is how the two sub-issues above are filed, and it is the rule the implementing agent works under.
