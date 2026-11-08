//! Live activity: imports and update checks run as backend jobs, and the UI
//! polls them so you can watch work happen (and stop it) instead of waiting on
//! a blocked toolbar.

import { cancelJob, listJobs, type JobInfo } from "./api";

export const activity = $state<{ jobs: JobInfo[] }>({ jobs: [] });

/** Bumps whenever a job started/finished or its current item/progress moved.
 * Views subscribe with onActivityChange() to re-read the grid — which is what
 * makes items visibly arrive while an import is still running. */
export const activityTick = $state({ n: 0 });

let timer: ReturnType<typeof setInterval> | null = null;
let pollErrors = 0;
let lastJobsJson = "";
/** so we refresh again a beat after the last progress bump */
let lastNotifyAt = 0;
const listeners = new Set<() => void>();

export function onActivityChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function notify() {
  activityTick.n += 1;
  lastNotifyAt = Date.now();
  for (const fn of listeners) fn();
}

async function poll() {
  try {
    const jobs = await listJobs();
    pollErrors = 0;
    const json = JSON.stringify(jobs);
    if (json !== lastJobsJson) {
      lastJobsJson = json;
      activity.jobs = jobs;
      notify();
    } else if (jobs.some((j) => !j.finished) && Date.now() - lastNotifyAt > 1500) {
      // long fetch pauses between items — still refresh the grid periodically
      notify();
    }
  } catch {
    // Vault may be locked/nuke'd while polling; stop hammering if it keeps failing.
    pollErrors += 1;
    if (pollErrors > 5 && timer !== null) {
      clearInterval(timer);
      timer = null;
      activity.jobs = [];
    }
  }
}

export function startWatching() {
  if (timer !== null) return;
  void poll();
  // 800 ms while something runs; slows to ~4 s while idle so a long-running
  // app doesn't burn cycles, but a job started from elsewhere is still seen.
  let idleTicks = 0;
  timer = setInterval(() => {
    if (activity.jobs.some((j) => !j.finished)) {
      idleTicks = 0;
      void poll();
      return;
    }
    idleTicks += 1;
    if (idleTicks <= 1 || idleTicks % 5 === 0) void poll();
  }, 800);
}

export function stopWatching() {
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

export function stopJob(id: number) {
  void cancelJob(id).catch(() => {});
}

/** "running" | "finished" | "cancelled" | "idle" per job kind, for buttons
 * that must not start a second job of the same kind. */
export function jobState(kind: string): "running" | "cancelled" | "finished" | "idle" {
  const j = activity.jobs.filter((x) => x.kind === kind).at(-1);
  if (!j) return "idle";
  if (!j.finished) return j.cancelled ? "cancelled" : "running";
  return j.cancelled ? "cancelled" : "finished";
}

export function jobLabel(j: JobInfo): string {
  const kind =
    j.kind === "bookmarks"
      ? "bookmarks"
      : j.kind === "games"
        ? "games"
        : j.kind === "images"
          ? "images"
          : j.kind === "videos"
            ? "videos"
            : j.kind === "enrich"
              ? "re-fetching"
              : "updates";
  if (j.finished) return `${kind}: ${j.note || "done"}`;
  if (j.cancelled) return `${kind}: stopping…`;
  if (j.total > 0) return `${kind} ${j.done}/${j.total}`;
  return `${kind}: ${j.note || j.current || "working"}`;
}
