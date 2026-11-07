<script lang="ts">
  import { unlockVault, type AppStatus } from "../lib/api";

  let { status, refresh }: { status: AppStatus; refresh: () => Promise<void> } = $props();

  let passphrase = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let input = $state<HTMLInputElement | null>(null);

  $effect(() => {
    input?.focus();
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      await unlockVault(passphrase);
      passphrase = "";
      await refresh();
    } catch (err) {
      error = String(err);
      passphrase = "";
    } finally {
      busy = false;
    }
  }
</script>

<div class="center-screen">
  <form class="card" onsubmit={submit}>
    <h1>🔒 Vault locked</h1>
    <p class="sub path">Enter passphrase to decrypt · Vault: {status.vaultRoot}</p>

    <label for="pw">Passphrase</label>
    <input
      id="pw"
      type="password"
      bind:this={input}
      bind:value={passphrase}
      autocomplete="off"
      disabled={busy}
    />

    <div style="margin-top: 1.25rem">
      <button class="primary" type="submit" disabled={busy || !passphrase}>
        {busy ? "Deriving key…" : "Unlock"}
      </button>
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}
  </form>
</div>
