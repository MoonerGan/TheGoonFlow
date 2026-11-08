<script lang="ts">
  // Shared editor, but pipeline fields (status/version/circle) only render for
  // games — a booru post has no "wishlist" state, and pretending otherwise
  // made the Media view confusing.
  import { onMount } from "svelte";
  import {
    addItemTag,
    deleteItem,
    getItem,
    removeItemTag,
    setComplete,
    updateItem,
    type ItemDetail,
    type ItemSummary,
  } from "../lib/api";

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
      await updateItem(item.id, title, circle || null, status, version || null);
      await setComplete(item.id, complete);
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

  async function remove() {
    if (!confirm(`Remove "${item.title}" from the vault? Files on disk are not touched.`)) return;
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
    {/if}

    <label class="inline-check">
      <input type="checkbox" bind:checked={complete} />
      ✓ Complete <span class="hint">(personal marker — never hides an update)</span>
    </label>

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

    {#if detail && detail.sources.length > 0}
      <div class="field-label">Sources</div>
      {#each detail.sources as s (s.id)}
        <div class="source-row">
          <span class="chip">{s.site}</span>
          <code>{s.url}</code>
        </div>
      {/each}
    {/if}

    <div class="row modal-actions">
      <button class="primary" disabled={saving} onclick={save}>Save</button>
      <button class="ghost" onclick={onClose}>Cancel</button>
      <span class="spacer"></span>
      <button class="danger" onclick={remove}>Remove from vault</button>
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>
</div>
