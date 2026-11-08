<script lang="ts">
  // #21: full in-app viewer for media items. Clicking a media card opens this
  // instead of the editor (edit is one button away). Full-res image bytes come
  // straight from the encrypted vault — never from a re-download.
  import { onMount } from "svelte";
  import { getMediaView, type ItemSummary, type MediaView as MediaBlob } from "../lib/api";

  let {
    items,
    index,
    onClose,
    onEdit,
  }: {
    items: ItemSummary[];
    index: number;
    onClose: () => void;
    onEdit: (item: ItemSummary) => void;
  } = $props();

  // The viewer is mounted per click, so capturing the initial index is intended.
  // svelte-ignore state_referenced_locally
  let idx = $state(index);
  let blob = $state<MediaBlob | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let zoom = $state(1); // 1 = fit-to-window
  let slideshow = $state(false);

  const item = $derived(items[idx]);

  // #34: views re-load their item list whenever any job ticks, handing us a
  // NEW array with the same items. Keying on the primitive id means "same
  // picture" never re-triggers a fetch (that was the flicker), and the old
  // frame stays visible while the next one loads.
  const itemId = $derived(item?.id ?? null);
  const nextId = $derived(items[(idx + 1) % items.length]?.id ?? null);

  function load(id: string, prefetch: string | null) {
    loading = true;
    err = null;
    zoom = 1;
    getMediaView(id)
      .then((b) => (blob = b))
      .catch((e) => {
        blob = null;
        err = String(e);
      })
      .finally(() => (loading = false));
    // warm the next one so arrow-through-library feels instant
    if (prefetch) void getMediaView(prefetch).catch(() => {});
  }

  $effect(() => {
    if (itemId) load(itemId, nextId);
  });

  function move(d: number) {
    if (d > 0) idx = idx + 1 < items.length ? idx + 1 : 0; // slideshow wraps
    else idx = idx - 1 >= 0 ? idx - 1 : items.length - 1;
  }

  function onkey(e: KeyboardEvent) {
    switch (e.key) {
      case "Escape":
        onClose();
        break;
      case "ArrowRight":
        move(1);
        break;
      case "ArrowLeft":
        move(-1);
        break;
      case " ":
        e.preventDefault();
        slideshow = !slideshow;
        break;
      case "+":
      case "=":
        zoom = Math.min(8, zoom * 1.25);
        break;
      case "-":
        zoom = Math.max(0.2, zoom / 1.25);
        break;
      case "0":
        zoom = 1;
        break;
      case "i":
        if (item) onEdit(item);
        break;
      case "f":
        toggleFull();
        break;
    }
  }

  // slideshow: 4 s per item, stops at the end… by wrapping (library loop)
  $effect(() => {
    if (!slideshow) return;
    const t = setInterval(() => move(1), 4000);
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

  const src = $derived(blob?.data ? `data:${blob.mime};base64,${blob.data}` : null);
  const isVideo = $derived(!!blob && blob.mime.startsWith("video/"));
  const isPoster = $derived(!!blob && blob.kind === "thumb" && !!blob.tooBig);
</script>

<svelte:window onkeydown={onkey} />

<div class="viewer" class:fullscreen={full} bind:this={root} tabindex="-1" role="dialog" aria-label="media viewer">
  <div class="viewer-stage">
    {#if src && isVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video src={src} controls loop autoplay></video>
    {:else if src}
      <img src={src} alt={item?.title ?? ""} style="transform: scale({zoom})" />
    {:else if loading}
      <div class="viewer-note">loading…</div>
    {:else if err}
      <div class="viewer-note">{err}</div>
    {:else}
      <div class="viewer-note">
        nothing viewable is stored for this item — it is tracked by link.
        <br />
        (Open its editor to see the source, or use *Store copy in vault* for videos.)
      </div>
    {/if}
    {#if isPoster}
      <div class="viewer-note poster-note">
        stored video is too big to preview here — open it from its source link
      </div>
    {/if}
  </div>

  <div class="viewer-bar">
    <div class="viewer-info">
      <strong>{item?.title ?? ""}</strong>
      <span class="hint">
        {idx + 1}/{items.length}
        {#if blob?.kind === "full"}· full-res in vault{:else if blob?.kind === "video"}· stored video{:else if blob}· preview only{/if}
      </span>
    </div>
    <div class="row">
      <button onclick={() => move(-1)} title="previous (←)">◀</button>
      <button onclick={() => move(1)} title="next (→)">▶</button>
      <button class:active={slideshow} onclick={() => (slideshow = !slideshow)} title="slideshow (space)">
        ▶▶ {slideshow ? "Stop" : "Slideshow"}
      </button>
      <span class="viewer-zoom">
        <button onclick={() => (zoom = Math.max(0.2, zoom / 1.25))} title="zoom out (-)">−</button>
        <button onclick={() => (zoom = 1)} title="fit (0)">{Math.round(zoom * 100)}%</button>
        <button onclick={() => (zoom = Math.min(8, zoom * 1.25))} title="zoom in (+)">+</button>
      </span>
      <button onclick={() => item && onEdit(item)} title="open the editor (i)">Edit</button>
      <button class:active={full} onclick={toggleFull} title="fullscreen (f)">⛶</button>
      <button class="ghost" onclick={onClose} title="close (Esc)">Close</button>
    </div>
  </div>
</div>
