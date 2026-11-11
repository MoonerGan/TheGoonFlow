//! #44: the viewer's in-memory media cache.
//!
//! `media_view` hands the webview the whole decrypted copy of an item: 10-40 MB
//! is normal and 96 MB is the backend's inline ceiling. The old viewer warmed
//! one neighbour and threw that payload away, so every arrow press - backwards
//! included - went back through the vault. This module keeps a bounded window
//! of payloads in RAM (LRU by byte count) and serves navigation from there.
//!
//! HARD PRIVACY RULE (#23 invariant): decrypted vault bytes must never outlive
//! the unlocked state. clearMediaCache() below is the only wipe, and
//! lib/session.ts is the only caller - Lock, Nuke and a detected panic-lock all
//! funnel through it. Nothing here touches disk: entries are plain Uint8Array
//! in the JS heap. No Blob / object URL, because Chromium may back large Blobs
//! with temporary files, and that is exactly what this rule forbids.

import { getMediaView, type MediaView } from "./api";

/** Hard ceiling for everything cached at once, in bytes: 200 MB.
 * That is a couple of full-res images plus the +/-2 prefetch window; a long
 * slideshow can never push the webview heap past it because putMedia() evicts
 * least-recently-used entries until the total fits. */
export const MEDIA_CACHE_LIMIT_BYTES = 200 * 1024 * 1024;

/** How far the viewer warms in both directions (i-2 .. i+2). */
export const PREFETCH_RADIUS = 2;

/** Speculative reads in flight at once. A jump cancels the queued ones, and
 * the item actually on screen never queues behind them (urgent flag). */
const MAX_IN_FLIGHT = 2;

export type MediaEntry = {
  id: string;
  /** what the inline copy is, straight from media_view; "none" = link-only item */
  kind: MediaView["kind"] | "none";
  mime: string;
  /** encoded file bytes for still images; decoded to an ImageBitmap on display.
   * ArrayBuffer-backed on purpose: a Blob is only ever born inside one decode
   * call, never kept, and never given an object URL. */
  bytes: Uint8Array<ArrayBuffer> | null;
  /** base64 as it came back, kept only for <video> and gif: those elements want
   * a URL, so keeping the string beats paying to decode and re-encode it */
  raw: string | null;
  /** set when a stored video was too big to inline, so this is its poster */
  tooBig: number | null;
  /** what this entry costs against MEDIA_CACHE_LIMIT_BYTES */
  size: number;
};

/** Live stats for the UI. `epoch` bumps on every wipe so anything holding a
 * decoded frame can drop it in the same tick the vault stopped being open. */
export const mediaCache = $state({ bytes: 0, entries: 0, epoch: 0 });

const store = new Map<string, MediaEntry>();

type Waiter = {
  id: string;
  urgent: boolean;
  started: boolean;
  /** dropped (jumped past, or the vault was wiped): its answer is thrown away */
  dead: boolean;
  promise: Promise<MediaEntry | null>;
  resolve: (entry: MediaEntry | null) => void;
  reject: (reason: unknown) => void;
};

const waiting = new Map<string, Waiter>();

/** Rejection used when a queued speculative read is dropped because the user
 * moved on. Callers that show errors must ignore it (isCancelled). */
export class MediaCancelled extends Error {
  constructor(id: string) {
    super(`media read dropped: ${id}`);
    this.name = "MediaCancelled";
  }
}

export function isCancelled(reason: unknown): boolean {
  return reason instanceof MediaCancelled;
}

/** Cache hit, or an in-flight/coalesced request for this item. */
export function hasMedia(id: string): boolean {
  return store.has(id) || waiting.has(id);
}

/** Read one item: cache hit, existing request, or a new one.
 * `urgent` = the item on screen, so it starts straight away instead of
 * queueing behind speculative neighbour reads. */
export function loadMedia(id: string, opts?: { urgent?: boolean }): Promise<MediaEntry | null> {
  const hit = store.get(id);
  if (hit) return Promise.resolve(touch(id, hit));
  const busy = waiting.get(id);
  if (busy) {
    // the item on screen was already being warmed as a neighbour: it is no
    // longer allowed to queue behind other speculative reads
    if (opts?.urgent && !busy.urgent) {
      busy.urgent = true;
      pump();
    }
    return busy.promise;
  }
  let resolve!: (entry: MediaEntry | null) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<MediaEntry | null>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  waiting.set(id, { id, urgent: !!opts?.urgent, started: false, dead: false, promise, resolve, reject });
  pump();
  return promise;
}

/** Stops caring about one request. */
function drop(w: Waiter): void {
  w.dead = true;
  waiting.delete(w.id);
  w.reject(new MediaCancelled(w.id));
}

/** #44: the user jumped (arrow spam, slideshow skip). Anything still queued
 * outside the ids we now care about is dropped before it is ever fetched - an
 * `invoke` already in flight cannot be aborted, so its answer is discarded by
 * the caller's generation token instead. */
export function cancelOutsideWindow(keep: Set<string>): void {
  for (const w of [...waiting.values()]) {
    if (w.started || keep.has(w.id)) continue;
    drop(w);
  }
}

/** The one wipe. Vault locked, nuked or panic-hidden -> nothing decrypted is
 * kept anywhere on this side of the boundary. Reads still on the wire are
 * marked dead so their answers are thrown away instead of being cached. */
export function clearMediaCache(): void {
  for (const w of [...waiting.values()]) drop(w);
  store.clear();
  mediaCache.bytes = 0;
  mediaCache.entries = 0;
  mediaCache.epoch += 1;
}

/** #44 follow-up: drop one item's entry. Called when items are deleted, so the
 * bytes of something the user removed do not sit in the window's cache under an
 * id that no longer resolves (they would die at the next lock anyway, but a long
 * unlocked session should not pay for deleted items). */
export function evictMedia(ids: string[]): void {
  let changed = false;
  for (const id of ids) {
    if (store.delete(id)) changed = true;
    const w = waiting.get(id);
    if (w) {
      drop(w);
      changed = true;
    }
  }
  if (!changed) return;
  mediaCache.bytes = mediaCacheTotal();
  mediaCache.entries = store.size;
}

/** LRU touch: re-insert so iteration order is oldest-first again. */
function touch(id: string, entry: MediaEntry): MediaEntry {
  store.delete(id);
  store.set(id, entry);
  return entry;
}

function putMedia(entry: MediaEntry): void {
  // delete first: Map.set on an existing key keeps its old position, which
  // would leave a re-fetched item parked at the front of the LRU queue
  store.delete(entry.id);
  store.set(entry.id, entry);
  // A single item bigger than the whole budget is still handed to the caller,
  // just never kept: caching it would evict everything useful.
  if (entry.size > MEDIA_CACHE_LIMIT_BYTES) {
    store.delete(entry.id);
  } else {
    for (const oldest of store.keys()) {
      if (mediaCacheTotal() <= MEDIA_CACHE_LIMIT_BYTES) break;
      if (oldest === entry.id) continue;
      store.delete(oldest);
    }
  }
  mediaCache.bytes = mediaCacheTotal();
  mediaCache.entries = store.size;
}

function mediaCacheTotal(): number {
  let n = 0;
  for (const e of store.values()) n += e.size;
  return n;
}

function start(w: Waiter): void {
  w.started = true;
  void getMediaView(w.id)
    .then((v) => {
      // jumped past, or the vault was wiped while this was on the wire: the
      // payload is thrown away, never turned into an entry, never cached
      if (w.dead) throw new MediaCancelled(w.id);
      return toEntry(w.id, v);
    })
    .then((entry) => {
      if (w.dead) throw new MediaCancelled(w.id);
      if (entry) putMedia(entry);
      w.resolve(entry);
    })
    .catch((e) => {
      if (!w.dead) w.reject(e);
    })
    .finally(() => {
      if (waiting.get(w.id) === w) waiting.delete(w.id);
      pump();
    });
}

function pump(): void {
  let active = 0;
  for (const w of waiting.values()) if (w.started) active += 1;
  for (const w of waiting.values()) {
    if (w.started || !w.urgent) continue;
    start(w);
    active += 1;
  }
  for (const w of waiting.values()) {
    if (w.started || active >= MAX_IN_FLIGHT) continue;
    start(w);
    active += 1;
  }
}

async function toEntry(id: string, v: MediaView | null): Promise<MediaEntry | null> {
  if (!v) return { id, kind: "none", mime: "", bytes: null, raw: null, tooBig: null, size: 0 };
  const raw = v.data;
  if (!raw) return { id, kind: v.kind, mime: v.mime, bytes: null, raw: null, tooBig: v.tooBig, size: 0 };
  if (v.kind === "video" || v.mime.startsWith("video/") || v.mime === "image/gif") {
    // a base64 string costs about this much heap; counted honestly so the cap
    // still means something for stored videos
    return { id, kind: v.kind, mime: v.mime, bytes: null, raw, tooBig: v.tooBig, size: raw.length };
  }
  return decode(id, v, raw);
}

/** base64 -> bytes. The data: URL decode is native; walking 50M characters in
 * JS (atob + charCodeAt) cost hundreds of ms per big image. */
async function decode(id: string, v: MediaView, raw: string): Promise<MediaEntry> {
  const expected = Math.floor((raw.length * 3) / 4) - (raw.endsWith("==") ? 2 : raw.endsWith("=") ? 1 : 0);
  try {
    const buf = await (await fetch(`data:application/octet-stream;base64,${raw}`)).arrayBuffer();
    if (buf.byteLength === expected) {
      return { id, kind: v.kind, mime: v.mime, bytes: new Uint8Array(buf), raw: null, tooBig: v.tooBig, size: buf.byteLength };
    }
  } catch {
    // fetch of a data: url refused - fall through to the pure-JS decode
  }
  const bin = atob(raw);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) bytes[i] = bin.charCodeAt(i);
  return { id, kind: v.kind, mime: v.mime, bytes, raw: null, tooBig: v.tooBig, size: bytes.byteLength };
}
