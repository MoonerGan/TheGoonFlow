<script lang="ts">
  // One box for everything: booru post/gallery pages, e-hentai, DLsite, f95,
  // direct image/video URLs, and file:/// links to media on this machine.
  import { clipUrl, type ClipReport } from "../lib/api";

  let {
    onClose,
    onDone,
  }: {
    onClose: () => void;
    onDone: (r: ClipReport) => void;
  } = $props();

  let text = $state("");
  let error = $state<string | null>(null);
  let pending = $state(false);

  async function doClip() {
    const url = text.trim();
    if (!url || pending) return;
    pending = true;
    error = null;
    try {
      const r = await clipUrl(url);
      text = "";
      onClose();
      onDone(r);
    } catch (e) {
      error = String(e);
    } finally {
      pending = false;
    }
  }
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
>
  <div class="modal card" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
    <h1>Clip a link</h1>
    <p class="sub">One box, everything: booru post pages and file links, e-hentai galleries, DLsite products,
      f95 threads, and <code>file:///</code> links to media on this machine.</p>
    <textarea
      class="clip-box"
      placeholder="https://… or file:///D:/Media/clip.mp4"
      bind:value={text}
      onkeydown={(e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          doClip();
        }
      }}
    ></textarea>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <div class="row modal-actions">
      <button class="primary" disabled={pending || !text.trim()} onclick={doClip}>Clip</button>
      <button class="ghost" onclick={onClose}>Cancel</button>
    </div>
  </div>
</div>
