<script lang="ts">
  // #21: full in-app viewer for media items. Clicking a media card opens this
  // instead of the editor (edit is one button away). Full-res image bytes come
  // straight from the encrypted vault — never from a re-download.
  // #42: fullscreen means the picture only. The bar floats over it, hides after
  // 2 s without the pointer, and Esc still leaves fullscreen before it closes.
  // #44: navigation is served from a bounded in-memory cache, a i-2..i+2 window
  // is warmed in both directions, a jump drops the queued speculative reads,
  // the stored thumbnail is the stand-in, and the full copy is decoded off the
  // main thread (createImageBitmap -> bitmaprenderer) instead of a base64 <img>.
  // #62: an item can hold several pictures - a `+Images` folder import stores one
  // per file, #53's stored attachments do the same - so the viewer now walks
  // *inside* an item first: ←/→ step through its frames, and only at the edge do
  // they move to the next item. A strip under the stage shows where you are.
  import { onDestroy, onMount } from "svelte";
  import { type ItemSummary, type MediaFrame } from "../lib/api";
  import { toast } from "../lib/toast.svelte";
  import {
    clampZoom,
    fitScale,
    isFitted,
    isOneToOne,
    onScreenPercent,
    oneToOneZoom,
    type Size,
    zoomCeiling,
  } from "../lib/view-zoom";
  import {
    cancelOutsideWindow,
    hasMedia,
    isCancelled,
    loadFrames,
    loadMedia,
    mediaCache,
    PREFETCH_RADIUS,
    type MediaEntry,
  } from "../lib/mediacache.svelte";

  let {
    items,
    index,
    thumbs = {},
    onClose,
    onEdit,
  }: {
    items: ItemSummary[];
    index: number;
    /** the tile thumbnails, so the first frame is already in memory (#44) */
    thumbs?: Record<string, string>;
    onClose: () => void;
    onEdit: (item: ItemSummary) => void;
  } = $props();

  // The viewer is mounted per click, so capturing the initial index is intended.
  // svelte-ignore state_referenced_locally
  let idx = $state(index);
  // $state.raw: a 10-40 MB payload must not be walked by the deep proxy
  let entry = $state.raw<MediaEntry | null>(null);
  // #62: the pictures this item holds, and which one is on screen. `frames` is
  // metadata only (id, kind, mime, size), so a 40-picture gallery lists cheaply
  // and pulls one frame's bytes at a time.
  let frames = $state<MediaFrame[]>([]);
  let f = $state(0);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let zoom = $state(1); // 1 = fit-to-window
  let slideshow = $state(false);
  let frameReady = $state(false);

  const item = $derived(items[idx]);

  // #34: views re-load their item list whenever any job ticks, handing us a
  // NEW array with the same items. Keying on the primitive id means "same
  // picture" never re-triggers a fetch (that was the flicker), and the old
  // frame stays visible while the next one loads.
  const itemId = $derived(item?.id ?? null);
  const shown = $derived(frames[f] ?? null);

  // #44: the card's stored thumbnail is already in memory, so it stands in for
  // the full copy instead of a spinner, and is replaced when the real bytes land.
  const poster = $derived(itemId ? thumbs[itemId] ?? null : null);

  const isVideo = $derived(!!entry && entry.mime.startsWith("video/"));
  // animated gifs stay on the plain <img> path: createImageBitmap decodes frame
  // one only, and gif payloads are small enough for a data: url.
  const isGif = $derived(!!entry && entry.mime === "image/gif");
  const isStill = $derived(!!entry && !!entry.bytes && !isVideo && !isGif);
  const elementSrc = $derived(entry?.raw ? `data:${entry.mime};base64,${entry.raw}` : null);
  // #62: an oversized stored video keeps its own kind and reports the size it
  // could not inline. `INLINE_MAX` in src-tauri/src/lib.rs is where that line is
  // drawn, so the sentence states it instead of blaming the file.
  const tooBigMB = $derived(
    entry?.tooBig ? Math.max(1, Math.round(entry.tooBig / 1048576)) : 0,
  );
  /** the byte size of the frame on screen, for the notes that have to be specific */
  const shownMB = $derived(
    shown ? Math.max(1, Math.round(shown.size / 1048576)) : 0,
  );
  /**
   * Where the "too big to hand to the player" line is drawn: `INLINE_MAX` in
   * `src-tauri/src/lib.rs` (96 MB). It is repeated here so the stage can state
   * the number instead of blaming the file; changing one without the other makes
   * the sentence a lie, so both say where the other lives.
   */
  const INLINE_MAX_MB = 96;

  /** natural size of an <img> that just loaded (the tile stand-in, or a gif) */
  function sizeOf(el: EventTarget | null): Size | null {
    if (!(el instanceof HTMLImageElement) || !el.naturalWidth) return null;
    return { w: el.naturalWidth, h: el.naturalHeight };
  }

  /**
   * An item with no bytes in the vault, described accurately: the picture is a
   * link, a folder, or a video sitting on this machine. The old sentence told
   * people to "open it from its source link", which for a stored video is not
   * even true, and it never said that a video on disk cannot be played here yet.
   */
  const noMediaNote =
    "nothing viewable is stored for this item — it is kept as a link or as a path on this " +
    "machine, and its editor shows where. Press *Store* under Sources (one picture) or " +
    "*Store copy in vault* (a video) to bring bytes in; a video that stays on disk is not " +
    "played here yet.";

  // ---------- zoom ----------
  // The arithmetic lives in `lib/view-zoom.ts` so it can be run and asserted
  // (`node tools/view-zoom-check.js`). It used to live here as
  // `natural.w * zoom / mediaEl.offsetWidth` — with `mediaEl` written without its
  // `()`, so the divisor was `undefined`, the button set the scale to `NaN%`, and
  // the picture sat still while the bar read `NaN%` (2026-11-15 review).
  //
  // `natural` is the picture actually on screen: the stored tile while the full
  // copy loads, then the frame itself. `stageBox` is the box it is laid out in,
  // measured before any transform, refreshed by a ResizeObserver so it is also
  // right after a window resize or a fullscreen change.
  let natural = $state<Size | null>(null);
  let boxEl = $state<HTMLDivElement | null>(null);
  let stageBox = $state<Size | null>(null);

  $effect(() => {
    const el = boxEl;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      stageBox = { w: r.width, h: r.height };
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  });

  /** how much of one picture pixel lands on one screen pixel at zoom 1 */
  const fit = $derived(fitScale(natural, stageBox));
  const ceiling = $derived(zoomCeiling(natural, stageBox));
  /** the honest readout: the picture's own pixels, not the transform */
  const scalePct = $derived(onScreenPercent(zoom, natural, stageBox));
  const atFit = $derived(isFitted(zoom));
  const atOne = $derived(isOneToOne(zoom, natural, stageBox));
  /** Do we know how big this picture is at all? A frame that has not decoded yet,
   * or one whose bitmap came back empty, has no size to talk about - and the UI
   * has to say "not known" rather than print `0×0 px, shown at ?%`. */
  const sized = $derived(!!natural && natural.w > 0 && natural.h > 0);
  /**
   * The picture is no larger than the space the stage has, so there is nothing for
   * "Fit" to shrink and Fit / 1:1 are the same state. That is true by definition
   * (fit never enlarges a picture), and the UI says so: the 2026-11-22 report read
   * two lit buttons as a bug, and the honest answer is a sentence, not a guess.
   */
  const smallerThanStage = $derived(sized && fit >= 0.999);

  function setZoom(z: number) {
    zoom = clampZoom(z, ceiling);
  }

  /** zoom 1 = the whole picture inside the stage (the default, key `0`). */
  function fitToWindow() {
    zoom = 1;
  }

  /**
   * One picture pixel per screen pixel (key `a`). Always recomputed from the
   * frame and the stage rather than from where the zoom happens to be, so
   * pressing it twice lands in the same place. If the frame's own size is not
   * known yet the button says so out loud instead of moving the picture to NaN.
   */
  function actualPixels() {
    const target = oneToOneZoom(natural, stageBox);
    if (target <= 0) {
      toast("this picture's own size is not known yet, so 1:1 cannot be worked out", "warn");
      return;
    }
    zoom = clampZoom(target, ceiling);
  }

  /** the zoom buttons' shared tooltip: what this picture is, and where we are */
  const zoomHint = $derived(
    natural && natural.w > 0 && natural.h > 0
      ? `this frame is ${natural.w}×${natural.h} px · shown at ${scalePct ?? "?"}% of that` +
        ` · Fit (0) = the whole picture inside the window, never enlarged` +
        ` · 1:1 (a) = one picture pixel per screen pixel` +
        (smallerThanStage ? " · this picture is smaller than the window, so Fit already is 1:1" : "")
      : "this picture's own pixel size is not known yet, so Fit and 1:1 cannot be measured",
  );

  // #44: bumped on every navigation. A payload that lands after the user moved
  // on is dropped by this token; queued speculative reads are cancelled before
  // they are issued at all (an invoke already on the wire cannot be aborted).
  let nav = 0;

  /** Pull one frame's bytes. `mine` is the navigation token that still owns the
   * screen; a payload answering an older token is thrown away. */
  function openFrame(frameId: number | undefined, mine: number): Promise<void> {
    if (frameId === undefined) {
      entry = null; // the item holds no media at all: the link-only note shows
      loading = false;
      return Promise.resolve();
    }
    return loadMedia(frameId, { urgent: true })
      .then((e) => {
        if (mine !== nav) return;
        entry = e;
      })
      .catch((x) => {
        if (mine !== nav || isCancelled(x)) return;
        entry = null;
        err = String(x);
      })
      .finally(() => {
        if (mine === nav) loading = false;
      });
  }

  function openItem(id: string) {
    const mine = ++nav;
    loading = true;
    err = null;
    zoom = 1;
    f = 0;
    frames = [];
    loadFrames(id)
      .then((list) => {
        if (mine !== nav) return;
        frames = list;
        return openFrame(list[0]?.id, mine);
      })
      .catch((x) => {
        if (mine !== nav || isCancelled(x)) return;
        entry = null;
        err = String(x);
        loading = false;
      });
  }

  $effect(() => {
    if (itemId) openItem(itemId);
  });

  // #44: warm a window on both sides, so backwards is as instant as forwards.
  const windowIds = $derived.by(() => {
    const n = items.length;
    if (n === 0) return [];
    const ids: string[] = [];
    for (let d = -PREFETCH_RADIUS; d <= PREFETCH_RADIUS; d += 1) {
      if (d === 0) continue;
      const at = (((idx + d) % n) + n) % n;
      const id = items[at]?.id;
      if (id && id !== itemId && !ids.includes(id)) ids.push(id);
    }
    return ids;
  });

  /** #62: the frames next to this one, inside the same item. Cheap to know (the
   * list is already in memory) and the ones a step is most likely to want. */
  const nearFrames = $derived.by(() => {
    const here = shown?.id;
    const out: number[] = [];
    for (let i = Math.max(0, f - PREFETCH_RADIUS); i <= Math.min(frames.length - 1, f + PREFETCH_RADIUS); i += 1) {
      const fr = frames[i];
      if (fr && fr.id !== here) out.push(fr.id);
    }
    return out;
  });

  /** #62: warm a neighbour item's frame list (one small metadata read) and only
   * then its first frame, so the item next door costs one BLOB, not forty. */
  function warmItems(itemIds: string[]) {
    for (const id of itemIds) {
      loadFrames(id)
        .then((list) => {
          const first = list[0];
          if (!first || hasMedia(first.id)) return;
          return loadMedia(first.id).catch(() => {});
        })
        .catch(() => {});
    }
  }

  $effect(() => {
    const current = itemId;
    if (!current) return;
    const inside = nearFrames;
    // arrow spam / slideshow skip: anything queued outside the new window dies
    // before it can cost a 40 MB read
    cancelOutsideWindow(new Set(inside.map(String)));
    for (const id of inside) {
      if (hasMedia(id)) continue;
      loadMedia(id).catch(() => {});
    }
    warmItems(windowIds);
  });

  function move(d: number) {
    if (d > 0) idx = idx + 1 < items.length ? idx + 1 : 0; // slideshow wraps
    else idx = idx - 1 >= 0 ? idx - 1 : items.length - 1;
  }

  /** #62: ←/→ walk the pictures inside this item first, and only at the edge do
   * they leave for the next item - which starts at its first picture, because
   * guessing its last frame would mean reading a list we do not have yet. */
  function step(d: number) {
    const next = f + d;
    if (next >= 0 && next < frames.length) {
      f = next;
      const mine = ++nav;
      loading = true;
      err = null;
      zoom = 1;
      void openFrame(frames[next].id, mine);
      return;
    }
    move(d);
  }

  /** #62: jump straight to one picture of this item - the strip is clickable, so
   * a 40-picture gallery does not need 40 arrow presses. */
  function gotoFrame(i: number) {
    if (i === f || i < 0 || i >= frames.length) return;
    f = i;
    const mine = ++nav;
    loading = true;
    err = null;
    zoom = 1;
    void openFrame(frames[i].id, mine);
  }

  function onkey(e: KeyboardEvent) {
    if (full) showChrome();
    switch (e.key) {
      case "Escape":
        // #35 + #42: the first Esc belongs to the Fullscreen API (WebView2
        // leaves fullscreen before this handler sees the key); a second Esc —
        // arriving with no fullscreenElement — closes the viewer. The chrome
        // autohide below never touches Escape.
        if (document.fullscreenElement) return;
        onClose();
        break;
      case "ArrowRight":
        step(1);
        break;
      case "ArrowLeft":
        step(-1);
        break;
      case " ":
        e.preventDefault();
        slideshow = !slideshow;
        break;
      case "+":
      case "=":
        setZoom(zoom * 1.25);
        break;
      case "-":
        setZoom(zoom / 1.25);
        break;
      case "0":
        fitToWindow();
        break;
      case "a":
        actualPixels();
        break;
      case "i":
        if (item) onEdit(item);
        break;
      case "f":
        toggleFull();
        break;
    }
  }

  // slideshow: 4 s per picture (#62: per picture, not per item), wrapping through
  // the library - step() leaves the item at its last frame, so nothing is skipped
  $effect(() => {
    if (!slideshow) return;
    const t = setInterval(() => step(1), 4000);
    return () => clearInterval(t);
  });

  let root = $state<HTMLDivElement | null>(null);
  onMount(() => root?.focus());

  // #35: fullscreen mode (WebView2 supports the Fullscreen API). Esc leaves
  // fullscreen first (native behavior, synced via the event below); a second
  // Esc then closes the viewer.
  let full = $state(false);
  function toggleFull() {
    if (document.fullscreenElement) void document.exitFullscreen();
    else if (root) void root.requestFullscreen().catch(() => {});
  }
  $effect(() => {
    const h = () => (full = !!document.fullscreenElement);
    document.addEventListener("fullscreenchange", h);
    return () => document.removeEventListener("fullscreenchange", h);
  });

  // #42: the bar is an overlay in fullscreen and hides itself again after two
  // seconds of no pointer and no key. Any movement brings it back.
  const CHROME_HIDE_MS = 2000;
  let chromeUp = $state(true);
  let barHover = $state(false);
  let chromeTimer: ReturnType<typeof setTimeout> | undefined;

  function hideChromeSoon() {
    clearTimeout(chromeTimer);
    chromeTimer = setTimeout(() => {
      if (barHover) hideChromeSoon(); // pointer is on the bar: keep it usable
      else chromeUp = false;
    }, CHROME_HIDE_MS);
  }

  function showChrome() {
    chromeUp = true;
    hideChromeSoon();
  }

  $effect(() => {
    if (!full) {
      clearTimeout(chromeTimer);
      chromeUp = true;
      return;
    }
    showChrome();
    return () => clearTimeout(chromeTimer);
  });

  // ---------- image frames (#44) ----------
  let frame = $state<HTMLCanvasElement | null>(null);
  let paintSeq = 0;
  /** the canvas element that currently holds a transferred bitmap */
  let painted: HTMLCanvasElement | null = null;
  /** the bytes are stills-sized but the decoder said no (HEIF, a broken file) */
  let decodeFailed = $state(false);

  $effect(() => {
    const current = entry;
    const cv = frame;
    if (decodeFailed) decodeFailed = false;
    if (!current || !cv || !isStill || !current.bytes) return;
    const ctx = cv.getContext("bitmaprenderer");
    if (!ctx) {
      decodeFailed = true; // no bitmaprenderer: the stand-in stays on screen
      return;
    }
    if (cv !== painted) frameReady = false; // fresh element: nothing on it yet
    const mine = ++paintSeq;
    // createImageBitmap wants a Blob (the standard source, and the decode runs
    // off the main thread). The Blob is born and dies inside this call: it
    // never gets an object URL, and the cache itself stays a plain Uint8Array.
    const source = new Blob([current.bytes], { type: current.mime });
    createImageBitmap(source)
      .then((bmp) => {
        if (mine !== paintSeq || frame !== cv) {
          bmp.close(); // a newer navigation won while this was decoding
          return;
        }
        // The size has to be read BEFORE the transfer: `transferFromImageBitmap`
        // detaches the bitmap, and a detached ImageBitmap reports 0x0. Reading it
        // after (which is what shipped) made every full-res frame report 0x0, so
        // the fit factor was 0, 1:1 was disabled, the readout fell back to
        // "zoom 100%" and the info line said "0×0 px, shown at ?%" — measured live
        // on 2026-11-22 with tools\viewer-layout-probe.js (a 2400x1600 frame drawn
        // at 1085x723 reported no size at all).
        const w = bmp.width;
        const h = bmp.height;
        cv.width = w;
        cv.height = h;
        ctx.transferFromImageBitmap(bmp);
        painted = cv;
        frameReady = true;
        // the numbers the Fit / 1:1 buttons and the info line need
        natural = { w, h };
      })
      .catch(() => {
        // decoder refused this image: keep the stand-in rather than a spinner.
        // `decodeFailed` is what says so — the stage no longer guesses.
        if (mine === paintSeq) decodeFailed = true;
      });
  });

  // ---------- what may be said about this frame ----------
  // One of the stage's branches can put this frame on screen?
  const onStage = $derived(
    (isVideo && !!elementSrc) || (isGif && !!elementSrc) || isStill,
  );

  // Anything the stage *cannot* show is a toast, not a line inside the stage: the
  // review measured the note taking horizontal space from the picture it described
  // (and it was being shown for every picture, because the old condition tested
  // `shown` — the frame's metadata row — which is always present).
  const warnedAbout = new Set<string>();
  $effect(() => {
    const fr = shown;
    const e = entry;
    if (!fr || loading || err) return;
    let line: string | null = null;
    if (e && e.tooBig !== null) {
      line =
        `this video is stored in the vault at ${tooBigMB} MB; this app hands its player at most ` +
        `${INLINE_MAX_MB} MB and cannot stream past that yet. The copy is not lost — the item's ` +
        `editor can drop it again with *Remove stored video*.`;
    } else if (!onStage) {
      // Nothing to show at all is the stage's own message, not a warning; the
      // two real cases (bytes the backend would not inline, a format this build
      // cannot decode) are the sentences below.
      if (!e || e.kind === "none") return;
      line = `this frame is stored as ${e.mime} (${shownMB} MB in the vault) and this build cannot show that format`;
    } else if (isStill && decodeFailed) {
      line =
        `the stored ${e?.mime ?? fr.mime} frame (${shownMB} MB) could not be decoded by this build, ` +
        `so the saved thumbnail is on screen instead`;
    }
    if (!line) return;
    // One sentence per frame per reason: walking a hundred pictures must not
    // queue a hundred toasts, and a real repeat still re-arms the timer.
    const key = `${fr.id}|${line}`;
    if (warnedAbout.has(key)) return;
    warnedAbout.add(key);
    if (warnedAbout.size > 80) warnedAbout.clear();
    toast(line, "warn");
  });

  /** Lets go of whatever the canvas is showing (TS lib has no reset() on the
   * bitmaprenderer context; a 0x0 backing store drops the frame). */
  function dropFrame() {
    paintSeq += 1;
    if (frame) {
      frame.width = 0;
      frame.height = 0;
    }
    painted = null;
    frameReady = false;
    // no picture on screen, so no picture size to claim
    natural = null;
  }

  // #44 privacy: if the cache is wiped under this view (Lock, Nuke, panic-hidden
  // window), the frame it holds goes with it. Asking again is correct either
  // way: a locked vault answers "vault is locked", a merely minimised one just
  // refills from the still-open vault.
  let cacheEpoch = mediaCache.epoch;
  $effect(() => {
    if (mediaCache.epoch === cacheEpoch) return;
    cacheEpoch = mediaCache.epoch;
    dropFrame();
    entry = null;
    err = null;
    if (itemId) openItem(itemId);
  });

  onDestroy(() => {
    clearTimeout(chromeTimer);
    dropFrame();
    entry = null;
  });
</script>

<svelte:window onkeydown={onkey} />

<div
  class="viewer"
  class:fullscreen={full}
  class:chrome-down={full && !chromeUp}
  bind:this={root}
  tabindex="-1"
  role="dialog"
  aria-label="media viewer"
  onpointermove={() => full && showChrome()}
>
  <div class="viewer-stage">
    <!-- The media always sits inside a box that already fills the stage, so the
         strip and the control bar stop moving when a picture lands: the stage
         used to be sized by whatever happened to be on it, so a card with a
         360 px tile opened into a stage that grew twice in the first second. -->
    <div class="frame-box" bind:this={boxEl}>
      {#if isVideo && elementSrc}
        <!-- svelte-ignore a11y_media_has_caption -->
        <video src={elementSrc} controls loop autoplay></video>
      {:else if isGif && elementSrc}
        <img
          src={elementSrc}
          alt={item?.title ?? ""}
          style="transform: scale({zoom})"
          onload={(e) => (natural = sizeOf(e.currentTarget))}
        />
      {:else if isStill}
        {#if poster && !frameReady}
          <img
            class="frame-poster"
            src={poster}
            alt={item?.title ?? ""}
            onload={(e) => (natural = sizeOf(e.currentTarget))}
            style="transform: scale({zoom})"
          />
        {/if}
        <canvas
          class="viewer-frame"
          class:shown={frameReady}
          bind:this={frame}
          style="transform: scale({zoom})"
        ></canvas>
      {:else if poster && !err}
        <img
          class="frame-poster"
          src={poster}
          alt={item?.title ?? ""}
          onload={(e) => (natural = sizeOf(e.currentTarget))}
          style="transform: scale({zoom})"
        />
      {/if}
    </div>

    <!-- Stage text floats over the stage (see .viewer-note): a sentence must
         never take horizontal space away from the picture it is talking about.
         Anything that is a *warning* rather than the stage's own content is a
         toast instead, and disappears by itself. -->
    {#if loading}
      <div class="viewer-note stage-note">loading…</div>
    {:else if err}
      <div class="viewer-note stage-note">{err}</div>
    {:else if !onStage && (!entry || entry.kind === "none")}
      <div class="viewer-note">{noMediaNote}</div>
    {/if}
  </div>

  {#if frames.length > 1}
    <!-- #62: every picture this item holds. Chips rather than real thumbnails on
         purpose: the vault keeps one tile per item (#54), so 40 tiles would mean
         decoding 40 full-res copies to paint 40 small squares. The title says what
         each frame is, including its size in the vault. -->
    <div class="viewer-strip" role="group" aria-label="pictures in this item">
      {#each frames as fr, i (fr.id)}
        <button
          class="strip-tile"
          class:here={i === f}
          onclick={() => gotoFrame(i)}
          title={`${fr.kind} · ${fr.mime} · ${Math.max(1, Math.round(fr.size / 1024))} KB in the vault`}
          aria-label={`picture ${i + 1} of ${frames.length}`}
        >{i + 1}</button>
      {/each}
      <span class="strip-hint">
        {frames.length} pictures in this item — ← / → step through them, then the next item
      </span>
    </div>
  {/if}

  <div
    class="viewer-bar"
    role="group"
    aria-label="viewer controls"
    onpointerenter={() => (barHover = true)}
    onpointerleave={() => {
      barHover = false;
      if (full) hideChromeSoon();
    }}
  >
    <div class="viewer-info">
      <strong>{item?.title ?? ""}</strong>
      <span class="hint">
        {idx + 1}/{items.length}
        {#if frames.length > 1}· picture {f + 1}/{frames.length}{/if}
        {#if shown?.kind === "full"}· full-res in vault{:else if shown?.kind === "video"}· stored video{:else if shown}· preview only{/if}
        {#if shown}· {shown.mime}{/if}
        {#if natural && natural.w > 0 && natural.h > 0}· {natural.w}×{natural.h} px, shown at {scalePct ?? "?"}%{#if smallerThanStage} · Fit and 1:1 are the same here: the picture is smaller than the window{/if}{/if}
        {#if mediaCache.entries > 0}· {mediaCache.entries} in cache · {Math.round(mediaCache.bytes / 1048576)} MB{/if}
      </span>
    </div>
    <div class="row">
      <button onclick={() => step(-1)} title="previous picture, then the previous item (←)">◀</button>
      <button onclick={() => step(1)} title="next picture, then the next item (→)">▶</button>
      <button class:active={slideshow} onclick={() => (slideshow = !slideshow)} title="slideshow (space)">
        ▶▶ {slideshow ? "Stop" : "Slideshow"}
      </button>
      <span class="viewer-zoom" role="group" aria-label="zoom">
        <button onclick={() => setZoom(zoom / 1.25)} title="zoom out (−)">−</button>
        <button
          class:active={atFit}
          aria-pressed={atFit}
          onclick={fitToWindow}
          title="Fit: the whole picture inside the window (0)"
        >
          Fit
        </button>
        <button
          class:active={atOne}
          aria-pressed={atOne}
          disabled={!sized}
          onclick={actualPixels}
          title={smallerThanStage
            ? `1:1: one screen pixel per picture pixel — this frame is ${natural?.w}×${natural?.h} px and already fits the window, so Fit has it at 1:1 already (a)`
            : sized
              ? `1:1: one screen pixel per picture pixel — this frame is ${natural?.w}×${natural?.h} px (a)`
              : "1:1 needs the picture's own pixel size, which is not known yet"}
        >
          1:1
        </button>
        <button onclick={() => setZoom(zoom * 1.25)} title="zoom in (+)">+</button>
        <!-- The number is the picture's scale against its own pixels, never the
             transform: "100%" used to be a button label that meant "fit", which is
             how the 2026-11-15 "Actual is NaN%" report and its follow-up both read
             as broken. Unknown is shown as "?" and explained on hover. -->
        <span
          class="zoom-readout"
          class:unknown={scalePct === null}
          title={zoomHint}
          aria-label="scale against the picture's own pixels"
        >
          {scalePct === null ? "?" : `${scalePct}%`}
        </span>
      </span>
      <button onclick={() => item && onEdit(item)} title="open the editor (i)">Edit</button>
      <button class:active={full} onclick={toggleFull} title="fullscreen (f)">⛶</button>
      <button class="ghost" onclick={onClose} title="close (Esc)">Close</button>
    </div>
  </div>
</div>
