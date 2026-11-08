<script lang="ts">
  import { onMount } from "svelte";
  import { getStatus, lockVault, type AppStatus } from "./lib/api";
  import SetupView from "./views/SetupView.svelte";
  import UnlockView from "./views/UnlockView.svelte";
  import LibraryView from "./views/LibraryView.svelte";

  let status = $state<AppStatus | null>(null);
  let error = $state<string | null>(null);

  async function refresh() {
    try {
      status = await getStatus();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function doLock() {
    await lockVault();
    await refresh();
  }

  onMount(refresh);
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
