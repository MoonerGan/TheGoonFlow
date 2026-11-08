<script lang="ts">
  // Shared editor. Pipeline fields (status/version/circle/complete) render only
  // for games — a booru post has no "wishlist" state and no "I finished it"
  // marker (D14 + field report #2: "Complete has nothing to do with them").
  // M2.6 also adds the source editor: attach/remove source links and re-fetch
  // metadata, so a folder-only game can be pointed at its DLsite/f95 page.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addItemTag,
    addMod,
    addSource,
    applyMod,
    deleteItem,
    deleteMod,
    getItem,
    launchItem,
    launchMod,
    listMods,
    previewLaunch,
    removeItemTag,
    removeSource,
    reEnrichItem,
    setComplete,
    setLaunchPath,
    removeStoredVideo,
    revertMod,
    extractGame,
    extractPlan,
    storeVideo,
    updateItem,
    videoInfo,
    type EnrichReport,
    type ExtractPlan,
    type ItemDetail,
    type ItemSummary,
    type LaunchPlan,
    type ModRow,
    type VideoInfo,
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

  // launch (#18, games only)
  let launchPlan = $state<LaunchPlan | null>(null);
  let launching = $state(false);
  let launchMsg = $state<string | null>(null);

  // archive extraction + folder linking (#17, games only)
  let xplan = $state<ExtractPlan | null>(null);
  let extracting = $state(false);
  let confirmExtract = $state(false);

  // mods / tool recipes (#25, games only)
  let mods = $state<ModRow[]>([]);
  let modKind = $state<"mtool" | "cheat" | "other" | "dll">("mtool");
  let modName = $state("");
  let modPath = $state("");
  let modArgs = $state("");
  let modMsg = $state<string | null>(null);
  let launchingMod = $state<number | null>(null);

  // vault video storage (#16, media only, strictly opt-in)
  let vinfo = $state<VideoInfo | null>(null);
  let storing = $state(false);
  let videoMsg = $state<string | null>(null);
  let confirmStore = $state(false);

  onMount(async () => {
    try {
      const d = await getItem(item.id);
      if (d) {
        detail = d;
        circle = d.circle ?? "";
        complete = d.tags.some((t) => t.name === "complete" && t.typeKey === "user_status");
      }
      if (isGame) {
        try {
          launchPlan = await previewLaunch(item.id);
        } catch {
          // launch preview is advisory; ignore
        }
        try {
          xplan = await extractPlan(item.id);
        } catch {
          // advisory too
        }
        try {
          mods = await listMods(item.id);
        } catch {
          // advisory too
        }
      } else {
        try {
          vinfo = await videoInfo(item.id);
        } catch {
          // advisory too
        }
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

  async function doLaunch() {
    if (launching) return;
    launching = true;
    launchMsg = null;
    try {
      const r = await launchItem(item.id);
      launchPlan = r;
      launchMsg = r.ok ? `launched${r.pid ? ` (pid ${r.pid})` : ""} — ${r.exe}` : r.note;
    } catch (e) {
      launchMsg = String(e);
    } finally {
      launching = false;
    }
  }

  async function chooseLauncher() {
    const path = await open({
      title: "Pick this game's launcher .exe",
      filters: [{ name: "Executable", extensions: ["exe"] }],
    });
    if (typeof path !== "string") return;
    try {
      await setLaunchPath(item.id, path);
      launchPlan = await previewLaunch(item.id);
      launchMsg = "saved as this game's launcher";
    } catch (e) {
      launchMsg = String(e);
    }
  }

  async function clearLauncher() {
    try {
      await setLaunchPath(item.id, null);
      launchPlan = await previewLaunch(item.id);
      launchMsg = "launcher override cleared";
    } catch (e) {
      launchMsg = String(e);
    }
  }

  async function doExtract() {
    if (extracting) return;
    extracting = true;
    try {
      const r = await extractGame(item.id);
      launchMsg =
        `extracted ${r.entries} entries → ${r.dest}` +
        (r.sourceAdded ? " · folder linked as a source" : "") +
        (r.note ? ` · ${r.note}` : "");
      launchPlan = await previewLaunch(item.id);
      xplan = await extractPlan(item.id);
      onChanged();
    } catch (e) {
      launchMsg = String(e);
    } finally {
      extracting = false;
    }
  }

  async function linkGameFolder() {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir !== "string") return;
    try {
      await addSource(item.id, `file:///${dir.replace(/\\/g, "/")}`);
      launchMsg = `linked folder ${dir}`;
      launchPlan = await previewLaunch(item.id);
      onChanged();
    } catch (e) {
      launchMsg = String(e);
    }
  }

  const KIND_HINT: Record<string, string> = {
    mtool: "starts the tool with this game's exe passed to it (MTool's inject flow)",
    cheat: "starts the tool; attach to the game inside it (Cheat Engine)",
    other: "starts the tool with the arguments below ({game} = the game's exe)",
    dll: "deployed into the game folder on Apply — originals kept as .flowbak, one-click Revert",
  };

  async function pickModPayload(asFolder: boolean) {
    const p = asFolder
      ? await open({ multiple: false, directory: true })
      : await open({
          multiple: false,
          filters:
            modKind === "dll"
              ? [{ name: "Mod payload", extensions: ["zip", "dll"] }]
              : [{ name: "Executable", extensions: ["exe"] }],
        });
    if (typeof p === "string") modPath = p;
  }

  async function doAddMod() {
    modMsg = null;
    try {
      await addMod(item.id, modName, modKind, modPath, modArgs);
      mods = await listMods(item.id);
      modName = modPath = modArgs = "";
      modMsg = "recipe added";
      onChanged();
    } catch (e) {
      modMsg = String(e);
    }
  }

  async function doDeleteMod(m: ModRow) {
    modMsg = null;
    try {
      await deleteMod(item.id, m.id);
      mods = await listMods(item.id);
    } catch (e) {
      modMsg = String(e);
    }
  }

  async function doLaunchMod(m: ModRow) {
    if (launchingMod !== null) return;
    launchingMod = m.id;
    modMsg = null;
    try {
      const r = await launchMod(item.id, m.id);
      modMsg = `started ${m.name}` + (r.note ? ` · ${r.note}` : "");
    } catch (e) {
      modMsg = String(e);
    } finally {
      launchingMod = null;
    }
  }

  async function doApplyMod(m: ModRow) {
    modMsg = null;
    try {
      const r = await applyMod(item.id, m.id);
      mods = await listMods(item.id);
      modMsg = `deployed ${m.name} — originals kept as .flowbak`;
      void r;
      onChanged();
    } catch (e) {
      modMsg = String(e);
    }
  }

  async function doRevertMod(m: ModRow) {
    modMsg = null;
    try {
      const notes = await revertMod(item.id, m.id);
      mods = await listMods(item.id);
      modMsg = notes.length ? `reverted with notes: ${notes.join("; ")}` : `reverted ${m.name}`;
      onChanged();
    } catch (e) {
      modMsg = String(e);
    }
  }

  function mb(n: number | null): string {
    return n ? `${(n / (1024 * 1024)).toFixed(1)} MB` : "unknown size";
  }

  async function doStoreVideo() {
    if (storing) return;
    storing = true;
    videoMsg = "storing… (watch the activity strip)";
    try {
      const r = await storeVideo(item.id);
      videoMsg =
        `stored ${mb(r.storedBytes)} in the vault` +
        (r.thumbAdded ? " · first-frame thumbnail added" : "") +
        (r.note ? ` · ${r.note}` : "");
      vinfo = await videoInfo(item.id);
      onChanged();
    } catch (e) {
      videoMsg = String(e);
    } finally {
      storing = false;
    }
  }

  async function doRemoveVideo() {
    try {
      await removeStoredVideo(item.id);
      videoMsg = "stored copy removed — the link is untouched";
      vinfo = await videoInfo(item.id);
    } catch (e) {
      videoMsg = String(e);
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

    {#if isGame}
      <div class="field-label">
        Launch
        <span class="hint">— {launchPlan?.engine ? `detected ${launchPlan.engine}` : "start it from the library"}</span>
      </div>
      {#if launchPlan?.ok}
        <p class="hint path">{launchPlan.exe}</p>
      {:else if launchPlan?.note}
        <p class="hint">{launchPlan.note}</p>
      {/if}
      {#if xplan}
        <p class="hint">zip source: <span class="path">{xplan.zip}</span></p>
      {/if}
      <div class="row">
        <button class="primary" disabled={launching || !launchPlan?.ok} onclick={doLaunch}>
          {launching ? "Launching…" : "▶ Launch"}
        </button>
        {#if xplan && !launchPlan?.ok}
          <button class="primary" disabled={extracting} onclick={() => (confirmExtract = true)}>
            {extracting ? "Extracting… (see strip)" : "Extract zip…"}
          </button>
        {/if}
        <button onclick={linkGameFolder} title="Point the game at a folder on disk">
          Link folder…
        </button>
        <button onclick={chooseLauncher} title="If auto-detect picked the wrong .exe, point at the right one">
          Set launcher…
        </button>
        {#if detail?.launchPath}
          <button class="ghost" onclick={clearLauncher}>Clear override</button>
        {/if}
      </div>
      {#if launchMsg}
        <p class="report-inline">{launchMsg}</p>
      {/if}

      <div class="field-label" style="margin-top:1.2rem">
        Mods &amp; tools
        <span class="hint">— launch recipes only; the app never injects or patches anything</span>
      </div>
      {#if mods.length}
        {#each mods as m (m.id)}
          <div class="row mod-row">
            {#if m.kind === "dll"}
              {#if m.status === "applied"}
                <button class="primary" disabled={launchingMod !== null} onclick={() => doRevertMod(m)}>
                  ↺ Revert {m.name}
                </button>
                <span class="chip">deployed</span>
              {:else}
                <button class="primary" disabled={launchingMod !== null} onclick={() => doApplyMod(m)}>
                  Apply {m.name}
                </button>
                <span class="hint mod-kind">dll · not deployed</span>
              {/if}
            {:else}
              <button
                class="primary"
                disabled={launchingMod !== null}
                onclick={() => doLaunchMod(m)}
                title={KIND_HINT[m.kind]}
              >
                {launchingMod === m.id ? "Starting…" : `▶ ${m.name}`}
              </button>
              <span class="hint mod-kind">{m.kind}</span>
            {/if}
            <span class="hint path mod-path" title={m.path + (m.args ? ` ${m.args}` : "")}>
              {m.path}{#if m.args} <em>{m.args}</em>{/if}
            </span>
            <button class="ghost" onclick={() => doDeleteMod(m)} title="remove this recipe">×</button>
          </div>
        {/each}
      {:else}
        <p class="hint">Nothing registered — add MTool, Cheat Engine or any patcher tool for this game.</p>
      {/if}
      <p class="hint">{KIND_HINT[modKind]}</p>
      <div class="row">
        <select bind:value={modKind}>
          <option value="mtool">MTool recipe</option>
          <option value="cheat">Cheat Engine recipe</option>
          <option value="other">Other tool</option>
          <option value="dll">DLL / patch mod (apply/revert)</option>
        </select>
        <input type="text" placeholder="name (optional)" bind:value={modName} />
      </div>
      <div class="row">
        <input
          type="text"
          class="proxy-input"
          placeholder={modKind === "dll" ? "mod zip / .dll / folder path" : "tool .exe path"}
          bind:value={modPath}
        />
        <button onclick={() => pickModPayload(false)}>
          {modKind === "dll" ? "Zip/dll…" : "Exe…"}
        </button>
        {#if modKind === "dll"}
          <button onclick={() => pickModPayload(true)}>Folder…</button>
        {/if}
      </div>
      <div class="row">
        <input
          type="text"
          class="proxy-input"
          placeholder={'extra arguments, e.g. --target "{game}" --silent'}
          bind:value={modArgs}
          onkeydown={(e) => e.key === "Enter" && modPath && doAddMod()}
        />
        <button disabled={!modPath} onclick={doAddMod}>Add recipe</button>
      </div>
      {#if modMsg}
        <p class="report-inline">{modMsg}</p>
      {/if}
    {/if}

    {#if confirmExtract && xplan}
      <ConfirmDialog
        title="Extract archive"
        message={`Unpacks ${xplan.zip} into ${xplan.dest} and links that folder so the game can launch. The zip itself is never touched. If ${xplan.dest} already exists it is linked without re-extracting.`}
        confirmLabel="Extract"
        onConfirm={() => { confirmExtract = false; void doExtract(); }}
        onCancel={() => (confirmExtract = false)}
      />
    {/if}

    {#if !isGame && (vinfo?.hasSource || vinfo?.stored)}
      <div class="field-label">
        Video
        <span class="hint">— kept as a {vinfo?.sourceKind ?? "link"} by default</span>
      </div>
      {#if vinfo?.stored}
        <p class="hint path">encrypted copy in the vault · {mb(vinfo.storedSize)}</p>
        <div class="row">
          <button class="danger" onclick={doRemoveVideo}>Remove stored copy</button>
        </div>
      {:else}
        <p class="hint">source size: {mb(vinfo?.sourceSize ?? null)}</p>
        <div class="row">
          <button disabled={storing} onclick={() => (confirmStore = true)}>Store copy in vault</button>
        </div>
      {/if}
      {#if videoMsg}
        <p class="report-inline">{videoMsg}</p>
      {/if}
    {/if}

    {#if confirmStore}
      <ConfirmDialog
        title="Store video in the vault"
        message={`This copies ${mb(vinfo?.sourceSize ?? null)} into the encrypted database. The vault (and your backups) grow accordingly; Nuke destroys this copy with everything else. The original link stays either way.`}
        confirmLabel="Store copy"
        onConfirm={() => { confirmStore = false; void doStoreVideo(); }}
        onCancel={() => (confirmStore = false)}
      />
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
