<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    attachVault,
    createVault,
    getDefaultVaultDir,
    resetPointer,
  } from "../lib/api";
  import { nukeVaultWithErase } from "../lib/session";

  let { mode, refresh }: { mode: "create" | "attach"; refresh: () => Promise<void> } = $props();

  let dir = $state("");
  let passphrase = $state("");
  let confirmPass = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Nuke is available from this screen too (recovery path when a passphrase
  // is forgotten — the vault becomes a brick by design, not bypassable).
  // In attach mode, `dir` lets you nuke a vault the pointer lost track of.
  let nukeConfirm = $state("");
  let nukeError = $state<string | null>(null);

  $effect(() => {
    if (mode === "create" && dir === "") {
      getDefaultVaultDir().then((d) => (dir = d));
    }
  });

  async function pickFolder() {
    const selected = await open({ directory: true, defaultPath: dir || undefined });
    if (typeof selected === "string") {
      dir = selected;
      error = null;
    }
  }

  async function submit() {
    busy = true;
    error = null;
    try {
      if (mode === "create") {
        if (passphrase !== confirmPass) {
          throw "passphrases do not match";
        }
        await createVault(dir, passphrase);
      } else {
        await attachVault(dir);
      }
      await refresh();
    } catch (e) {
      error = String(e);
    } finally {
      passphrase = "";
      confirmPass = "";
      busy = false;
    }
  }

  async function doNuke() {
    nukeError = null;
    try {
      await nukeVaultWithErase(nukeConfirm, dir || undefined);
      dir = "";
      nukeConfirm = "";
      await refresh();
    } catch (e) {
      nukeError = String(e);
    }
  }

  async function doForget() {
    nukeError = null;
    try {
      await resetPointer();
      dir = "";
      await refresh();
    } catch (e) {
      nukeError = String(e);
    }
  }
</script>

<div class="center-screen">
  <div class="card">
    {#if mode === "create"}
      <h1>Create your vault</h1>
      <p class="sub">
        One folder holds everything — database, thumbnails, and later downloads.
        Put it beside the app, or on a USB drive you can carry. Passphrase is
        Argon2id-stretched and the database is SQLCipher; if you forget it,
        nothing recovers it.
      </p>

      <label for="vault-dir">Vault folder</label>
      <div class="row">
        <input id="vault-dir" type="text" bind:value={dir} spellcheck="false" />
        <button onclick={pickFolder}>Browse…</button>
      </div>

      <label for="pw">Passphrase</label>
      <input id="pw" type="password" bind:value={passphrase} autocomplete="new-password" />

      <label for="pw2">Confirm passphrase</label>
      <input id="pw2" type="password" bind:value={confirmPass} autocomplete="new-password" />

      <div style="margin-top: 1.25rem">
        <button class="primary" disabled={busy || !dir || !passphrase} onclick={submit}>
          {busy ? "Creating…" : "Create vault"}
        </button>
      </div>
    {:else}
      <h1>Reconnect your vault</h1>
      <p class="sub">
        The vault folder this app remembers isn't reachable right now. If it
        lives on a USB or external drive, plug it in — the volume label finds
        it. Otherwise point the app at the vault folder.
      </p>

      <label for="vault-dir">Vault folder</label>
      <div class="row">
        <input id="vault-dir" type="text" bind:value={dir} placeholder="D:\vaults\goonflow" spellcheck="false" />
        <button onclick={pickFolder}>Browse…</button>
      </div>

      <div style="margin-top: 1.25rem">
        <button class="primary" disabled={busy || !dir} onclick={submit}>Attach vault</button>
        <button class="ghost" style="margin-left: .5rem"
          title="Delete only this app's pointer file (flow.cfg). Touches no vault folder. Use it when the pointer points at a deleted or wrong folder and you want a clean start."
          onclick={doForget}>
          Forget pointer
        </button>
      </div>
    {/if}

    {#if error}
      <p class="error">{error}</p>
    {/if}

    <hr style="border: none; border-top: 1px solid var(--border); margin: 1.5rem 0" />

    <p class="hint">
      Forgotten passphrase? Crypto-erase the vault to start over. Every file is
      overwritten and deleted — irreversible. Type <strong>NUKE</strong> below to arm it.
      {#if mode === "attach"}
        If the folder you point at above is the vault, Nuke shreds that folder;
        with no folder entered it only clears this app's pointer.
      {/if}
    </p>
    <div class="row">
      <input type="text" bind:value={nukeConfirm} placeholder="NUKE" style="max-width: 120px" />
      <button class="danger" disabled={nukeConfirm !== "NUKE"} onclick={doNuke}>Nuke vault</button>
    </div>
    {#if nukeError}
      <p class="error">{nukeError}</p>
    {/if}
  </div>
</div>
