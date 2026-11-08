<script lang="ts">
  import { nukeVault, unlockVault, type AppStatus } from "../lib/api";

  let { status, refresh }: { status: AppStatus; refresh: () => Promise<void> } = $props();

  let passphrase = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let input = $state<HTMLInputElement | null>(null);

  // Forgotten-passphrase recovery: nuke is always reachable from the locked
  // screen, so a lost passphrase never hard-locks the app (you lose the
  // vault's contents, not the app).
  let showNuke = $state(false);
  let nukeConfirm = $state("");
  let nukeError = $state<string | null>(null);
  let nukeBusy = $state(false);

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

  async function doNuke() {
    nukeBusy = true;
    nukeError = null;
    try {
      await nukeVault(nukeConfirm);
      await refresh();
    } catch (e) {
      nukeError = String(e);
    } finally {
      nukeBusy = false;
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

    <hr style="border: none; border-top: 1px solid var(--border); margin: 1.5rem 0" />

    {#if !showNuke}
      <p class="hint">
        Forgotten the passphrase?
        <button class="link" type="button" onclick={() => (showNuke = true)}>
          Crypto-erase this vault
        </button>
        to start over — every file overwritten and deleted, irreversible.
      </p>
    {:else}
      <p class="hint">
        Type <strong>NUKE</strong> to crypto-erase this vault (irreversible).
      </p>
      <div class="row">
        <input type="text" bind:value={nukeConfirm} placeholder="NUKE" style="max-width: 120px" />
        <button class="danger" type="button" disabled={nukeConfirm !== "NUKE" || nukeBusy} onclick={doNuke}>
          {nukeBusy ? "Shredding…" : "Nuke vault"}
        </button>
      </div>
      {#if nukeError}
        <p class="error">{nukeError}</p>
      {/if}
    {/if}
  </form>
</div>
