// A warning that sits in the same space as the content is a design bug, and this
// app had two of them: a viewer note took horizontal space away from the picture
// it was talking about (#62's review), and the Sources cookie verdict painted
// itself over its own Set/Check/Clear buttons (#38's second review).
//
// So a message about *something that just happened* is a toast: it appears, names
// what happened, and leaves by itself. What stays in the panel is persistent
// *state* — a site is switched off, the panic key is not armed, a field is
// invalid — because that has to be readable after the moment passes.
//
// Privacy: a toast can name an item, a site or a path, so `endVaultSession()`
// clears the stack with the media cache. Nothing here is ever persisted.

export type ToastKind = "info" | "warn" | "bad";

export type Toast = {
  id: number;
  text: string;
  kind: ToastKind;
};

/** Long enough to read two sentences, short enough that nothing lingers. */
const DEFAULT_MS = 9000;
const WARN_MS = 12000;
/** A failing batch must be able to bury the screen; four rows is the cap. */
const MAX_ON_SCREEN = 4;

const live = $state<Toast[]>([]);
const timers = new Map<number, ReturnType<typeof setTimeout>>();
let seq = 0;

function remove(id: number): void {
  const t = timers.get(id);
  if (t !== undefined) clearTimeout(t);
  timers.delete(id);
  const at = live.findIndex((x) => x.id === id);
  if (at >= 0) live.splice(at, 1);
}

function arm(id: number, ms: number): void {
  const old = timers.get(id);
  if (old !== undefined) clearTimeout(old);
  // ms <= 0 means "stays until dismissed": for a message with an action in it.
  if (ms > 0) timers.set(id, setTimeout(() => remove(id), ms));
  else timers.delete(id);
}

/**
 * Show one line. The same sentence twice is one toast, re-timed rather than
 * stacked — a re-fetch of thirty items that all fail the same way says one thing,
 * and it should not fill the screen with it.
 */
export function toast(text: string, kind: ToastKind = "info", ms?: number): void {
  const line = String(text ?? "").trim();
  if (!line) return;
  const ttl = ms ?? (kind === "info" ? DEFAULT_MS : WARN_MS);
  const same = live.find((x) => x.text === line);
  if (same) {
    arm(same.id, ttl);
    return;
  }
  const id = ++seq;
  live.push({ id, text: line, kind });
  arm(id, ttl);
  while (live.length > MAX_ON_SCREEN) remove(live[0].id);
}

export const info = (text: string): void => toast(text, "info");
export const warn = (text: string): void => toast(text, "warn");
export const bad = (text: string): void => toast(text, "bad");
/** Stays until the user closes it (a warning that needs a decision). */
export const sticky = (text: string, kind: ToastKind = "warn"): void => toast(text, kind, 0);

export function dismissToast(id: number): void {
  remove(id);
}

export function clearToasts(): void {
  for (const id of [...live.map((x) => x.id)]) remove(id);
}

export const toasts = {
  get list(): Toast[] {
    return live;
  },
};
