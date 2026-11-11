//! Vault session guard.
//!
//! Project invariant (#23): decrypted vault bytes must never outlive the
//! unlocked state. Everything on this side of the IPC boundary that ends the
//! unlocked state - the Lock button, a Nuke, and the frontend noticing a
//! panic-key lock - funnels through endVaultSession(), which is the single
//! place that wipes the in-memory media cache (lib/mediacache.svelte.ts).
//!
//! How a Rust-side lock is noticed at all: the panic hotkey is handled entirely
//! in Rust (global shortcut -> `lock_core` + window hide) with no call into the
//! webview, so `lock_core`/`nuke_core` emit `vault-session-ended` and that is the
//! fast path. The status poll is the belt: it watches `app_status.generation`,
//! which the backend bumps on lock/nuke/attach, so even a hidden or reloading
//! webview that missed the event clears on its next read. The visibility hook is
//! the third net for the window-hidden-by-panic case. All three call the same
//! wipe, so no path can leak a second, forgetful copy of this logic.

import { getStatus, lockVault, nukeVault } from "./api";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { clearMediaCache } from "./mediacache.svelte";

/** Poll slow enough to be free, fast enough that re-showing a panic-hidden
 * window lands on the unlock screen instead of a stale library. */
const STATUS_POLL_MS = 2000;

let knownUnlocked = false;
let timer: ReturnType<typeof setInterval> | null = null;
let unlisten: UnlistenFn | null = null;
/** last `app_status.generation` we saw while unlocked; the backend bumps it on
 * lock, nuke and attach, so a jump means the cached bytes are stale even if the
 * unlocked flag somehow never flipped in front of us. */
let seenGeneration = -1;

/** The one clear path. */
export function endVaultSession(): void {
  clearMediaCache();
}

/** Seed with the state the app currently believes it is in; every route that
 * ends up on the unlock screen passes false, which wipes on its own. The
 * generation comes from the same `app_status` read, so re-unlocking re-seeds the
 * comparison instead of looking like a stale cache. */
export function noteVaultUnlocked(unlocked: boolean, generation?: number): void {
  if (!unlocked && knownUnlocked) endVaultSession();
  knownUnlocked = unlocked;
  if (typeof generation === "number") seenGeneration = generation;
}

/** Lock button (LibraryView header -> App.svelte). */
export async function lockVaultWithErase(): Promise<void> {
  try {
    await lockVault();
  } finally {
    // cleared even if the call failed: a failed lock is still a vault we must
    // not keep plaintext for
    endVaultSession();
  }
}

/** Nuke / crypto-erase, from the locked and setup screens. */
export async function nukeVaultWithErase(confirm: string, dir?: string): Promise<void> {
  try {
    await nukeVault(confirm, dir);
  } finally {
    endVaultSession();
  }
}

/** Watches for a lock nobody called us about (the panic key). Three independent
 * ways to notice, because the panic path runs entirely in Rust: the
 * `vault-session-ended` event the backend emits inside `lock_core`/`nuke_core`,
 * the status poll (which also catches a changed `generation`, i.e. a different
 * vault attached), and the window going hidden. `onLocked` only runs on a real
 * unlocked → locked transition, so views are not re-made on every poll. */
export function watchVaultSession(onLocked: () => void): () => void {
  if (timer !== null) return () => {};
  const lost = () => {
    if (!knownUnlocked) return;
    knownUnlocked = false;
    endVaultSession();
    onLocked();
  };
  const tick = async () => {
    if (!knownUnlocked) return;
    try {
      const s = await getStatus();
      const generationChanged = seenGeneration >= 0 && s.generation !== seenGeneration;
      seenGeneration = s.generation;
      if (generationChanged || !s.unlocked) lost();
    } catch {
      // backend busy or mid-erase: the next tick decides
    }
  };
  const onvisibility = () => {
    if (document.hidden) {
      // the panic key hides the window; while it is hidden nothing decrypted
      // may be held. Minimising costs a refetch, nothing else.
      if (knownUnlocked) endVaultSession();
    } else {
      void tick();
    }
  };
  void listen("vault-session-ended", () => {
    seenGeneration = -1; // the next status read re-seeds it
    lost();
  }).then((u) => (unlisten = u));
  timer = setInterval(() => void tick(), STATUS_POLL_MS);
  document.addEventListener("visibilitychange", onvisibility);
  return () => {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
    if (unlisten !== null) {
      unlisten();
      unlisten = null;
    }
    document.removeEventListener("visibilitychange", onvisibility);
  };
}
