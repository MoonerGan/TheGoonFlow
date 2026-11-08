<script lang="ts">
  // Shared editor. Pipeline fields (status/version/circle/complete) render only
  // for games — a booru post has no "wishlist" state and no "I finished it"
  // marker (D14 + field report #2: "Complete has nothing to do with them").
  // M2.6 also adds the source editor: attach/remove source links and re-fetch
  // metadata, so a folder-only game can be pointed at its DLsite/f95 page.
  import { onMount } from "svelte";
  import {
    addItemTag,
    addSource,
    deleteItem,
    getItem,
    removeItemTag,
    removeSource,
    reEnrichItem,
    setComplete,
    updateItem,
    type EnrichReport,
    type ItemDetail,
    type ItemSummary,
  } from "../lib/api";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  let {
    item,
    onClose,
    onChanged,
  }: {
    item: ItemSummary;
    onClose: () => void;
    onChanged: () => void;
  } = $props();

  const isGame = $derived(item.kind === "game");

  // The modal is mounted per item, so capturing the prop's initial values here
  // is exactly what we want.
  // svelte-ignore state_referenced_locally
  let title = $state(item.title);
  let circle = $state("");
  // svelte-ignore state_referenced_locally
  let status = $state(item.status);
  // svelte-ignore state_referenced_locally
  let version = $state(item.version ?? "");
  let complete = $state(false);
  let newTag = $state("");
  let error = $state<string | null>(null);
  let detail = $state<ItemDetail | null>(null);
  let saving = $state(false);
  let confirming = $state(false);

  // source editing
  let newSource = $state("");
  let sourceMsg = $state<string | null>(null);
  let enriching = $state(false);
  let enrichReport = $state<EnrichReport | null>(null);
  let enrichError = $state<string | null>(null);

  onMount(async () => {
    try {
      const d = await getItem(item.id);
      if (d) {
        detail = d;
        circle = d.circle ?? "";
        complete = d.tags.some((t) => t.name === "complete" && t.typeKey === "user_status");
      }
    } catch (e) {
      error = String(e);
    }
  });

  async function save() {
    if (saving) return;
    saving = true;
    error = null;
    try {
      // media has no real status — send the existing one through untouched
      await updateItem(item.id, title, circle || null, isGame ? status : item.status, version || null);
      await setComplete(item.id, isGame ? complete : false);
      onClose();
      onChanged();
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function addTag() {
    if (!newTag.trim()) return;
    try {
      await addItemTag(item.id, newTag.trim());
      newTag = "";
      detail = await getItem(item.id);
      onChanged();
    } catch (e) {
      error = String(e);
    }
  }

  async function dropTag(tagId: number) {
    try {
      await removeItemTag(item.id, tagId);
      detail = await getItem(item.id);
      onChanged();
    } catch (e) {
      error = String(e);
    }
  }

  async function attachSource() {
    const url = newSource.trim();
    if (!url) return;
    sourceMsg = null;
    try {
      const site = await addSource(item.id, url);
      sourceMsg = `linked to ${site} — press “Re-fetch metadata” to pull art/tags`;
      newSource = "";
      detail = await getItem(item.id);
      onChanged();
    } catch (e) {
      sourceMsg = String(e);
    }
  }

  async function detachSource(sourceId: number) {
    try {
      await removeSource(item.id, sourceId);
      detail = await getItem(item.id);
      onChanged();
    } catch (e) {
      sourceMsg = String(e);
    }
  }

  async function refetch() {
    if (enriching) return;
    enriching = true;
    enrichError = null;
    enrichReport = null;
    try {
      enrichReport = await reEnrichItem(item.id);
      detail = await getItem(item.id);
      onChanged();
    } catch (e) {
      enrichError = String(e);
    } finally {
      enriching = false;
    }
  }

  async function remove() {
    if (!confirming) {
      confirming = true;
      return;
    }
    try {
      await deleteItem(item.id);
      onClose();
      onChanged();
    } catch (e) {
      error = String(e);
    }
  }

  function when(epoch: string): string {
    const n = Number(epoch);
    return Number.isFinite(n) && n > 0 ? new Date(n * 1000).toLocaleString() : "—";
  }
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
>
  <div
    class="modal card"
    role="dialog"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <h1>Edit item</h1>
    <p class="sub">{item.kind} · imported {when(item.updatedAt)}</p>

    <label for="ed-title">Title</label>
    <input id="ed-title" type="text" bind:value={title} />

    {#if isGame}
      <label for="ed-circle">Circle / developer</label>
      <input id="ed-circle" type="text" bind:value={circle} />

      <div class="row two-col">
        <div>
          <label for="ed-status">Status</label>
          <select id="ed-status" bind:value={status}>
            {#each ["seen", "tracking", "wishlist", "downloaded", "played"] as s}
              <option value={s}>{s}</option>
            {/each}
          </select>
        </div>
        <div>
          <label for="ed-version">Version</label>
          <input id="ed-version" type="text" bind:value={version} placeholder="0.12" />
        </div>
      </div>

      <label class="inline-check">
        <input type="checkbox" bind:checked={complete} />
        ✓ Complete <span class="hint">(personal marker — never hides an update)</span>
      </label>
    {/if}

    <label for="ed-tag">Tags</label>
    <div class="chips">
      {#if detail}
        {#each detail.tags as t (t.id)}
          <span class="chip" class:chip-status={t.typeKey === "user_status"} class:chip-site={t.typeKey === "site_flag"}>
            {t.name}
            <button class="chip-x" title="remove" onclick={() => dropTag(t.id)}>×</button>
          </span>
        {/each}
      {/if}
    </div>
    <div class="row">
      <input id="ed-tag" type="text" placeholder="add a tag, press Enter" bind:value={newTag} onkeydown={(e) => e.key === "Enter" && addTag()} />
      <button onclick={addTag}>Add</button>
    </div>

    <div class="field-label">
      Sources
      <span class="hint">— add a DLsite / f95 / booru link to a folder-only entry, then re-fetch</span>
    </div>
    {#if detail && detail.sources.length > 0}
      {#each detail.sources as s (s.id)}
        <div class="source-row">
          <span class="chip">{s.site}</span>
          <code>{s.url}</code>
          <button class="chip-x" title="detach this source" onclick={() => detachSource(s.id)}>×</button>
        </div>
      {/each}
    {:else}
      <p class="hint">No sources yet.</p>
    {/if}
    <div class="row">
      <input type="text" placeholder="paste a source URL, press Enter" bind:value={newSource} onkeydown={(e) => e.key === "Enter" && attachSource()} />
      <button onclick={attachSource}>Link source</button>
    </div>
    {#if sourceMsg}
      <p class="report-inline">{sourceMsg}</p>
    {/if}

    <div class="row modal-actions">
      <button class="primary" disabled={saving} onclick={save}>Save</button>
      <button onclick={refetch} disabled={enriching}>{enriching ? "Re-fetching… (see strip)" : "Re-fetch metadata"}</button>
      <button class="ghost" onclick={onClose}>Cancel</button>
      <span class="spacer"></span>
      <button class="danger" onclick={remove}>Remove from vault</button>
    </div>

    {#if enrichReport}
      <p class="report-inline">
        {enrichReport.enriched}/{enrichReport.sources} sources gave something
        {#if enrichReport.stopped}(stopped){/if}
      </p>
      {#if enrichReport.failures.length > 0}
        <details class="details">
          <summary>{enrichReport.failures.length} source problem{enrichReport.failures.length === 1 ? "" : "s"}</summary>
          <ul>
            {#each enrichReport.failures as f}
              <li><code>{f.url}</code> — {f.reason}</li>
            {/each}
          </ul>
        </details>
      {/if}
      {#each enrichReport.notes as n}
        <p class="hint">{n}</p>
      {/each}
    {/if}
    {#if enrichError}
      <p class="error">{enrichError}</p>
    {/if}
    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>
</div>

{#if confirming}
  <ConfirmDialog
    title="Remove from vault"
    message={`Remove "${item.title}"? Entries and vault-stored images are deleted; files on your disk are never touched.`}
    confirmLabel="Remove"
    onConfirm={remove}
    onCancel={() => (confirming = false)}
  />
{/if}
