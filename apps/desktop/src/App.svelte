<script lang="ts">
  import { onMount } from "svelte";
  import { getStatus, type AppStatus } from "./lib/api";
  // #44 privacy: lock/nuke/panic all end the vault session through one place,
  // and that place is what wipes the viewer's decrypted media cache.
  import { lockVaultWithErase, noteVaultUnlocked, watchVaultSession } from "./lib/session";
  import SetupView from "./views/SetupView.svelte";
  import UnlockView from "./views/UnlockView.svelte";
  import LibraryView from "./views/LibraryView.svelte";

  let status = $state<AppStatus | null>(null);
  let error = $state<string | null>(null);

  async function refresh() {
    try {
      const s = await getStatus();
      status = s;
      noteVaultUnlocked(s.unlocked, s.generation);
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function doLock() {
    await lockVaultWithErase();
    await refresh();
  }

  onMount(() => {
    void refresh();
    // the panic hotkey locks from Rust without telling us; this notices
    return watchVaultSession(() => void refresh());
  });
</script>

{#if !status}
  <div class="center-screen">
    <p class="hint">Starting…</p>
  </div>
{:else if error}
  <div class="center-screen">
    <div class="card">
      <h1>Something went wrong</h1>
      <p class="error">{error}</p>
      <button class="primary" onclick={refresh}>Retry</button>
    </div>
  </div>
{:else if !status.hasVault}
  <SetupView mode="create" {refresh} />
{:else if !status.vaultResolved}
  <SetupView mode="attach" {refresh} />
{:else if !status.unlocked}
  <UnlockView {status} {refresh} />
{:else}
  <LibraryView {status} onLock={doLock} />
{/if}
