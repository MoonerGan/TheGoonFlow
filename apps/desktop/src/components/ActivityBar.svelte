<script lang="ts">
  // Live activity strip: what is running right now, which item it is on, and a
  // Stop button — no more frozen toolbar while something imports in the
  // background. Finished rows linger only briefly (issue #5: they used to pile up).
  import { onDestroy } from "svelte";
  import { activity, jobLabel, stopJob } from "../lib/activity.svelte";

  const SHOW_FINISHED_MS = 12_000;

  let now = $state(Date.now());
  let timer: ReturnType<typeof setInterval> | null = null;

  const live = $derived(activity.jobs.filter((j) => !j.finished));
  const recent = $derived(
    activity.jobs
      .filter((j) => j.finished && now - j.finishedAt * 1000 < SHOW_FINISHED_MS)
      .slice(-3)
      .reverse(),
  );

  // only tick while there is something to expire
  $effect(() => {
    if (recent.length === 0) {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
      return;
    }
    if (timer === null) timer = setInterval(() => (now = Date.now()), 2000);
  });

  onDestroy(() => {
    if (timer !== null) clearInterval(timer);
  });
</script>

{#if live.length > 0 || recent.length > 0}
  <div class="activity" class:activity-live={live.length > 0}>
    {#each live as j (j.id)}
      <div class="activity-row">
        <div class="activity-main">
          <span class="activity-label">
            <strong>{jobLabel(j)}</strong>
            {#if j.note}<span class="hint">{j.note}</span>{/if}
          </span>
          {#if j.total > 0}
            <span class="bar">
              <span class="bar-fill" style="width: {Math.min(100, Math.round((j.done / j.total) * 100))}%"></span>
            </span>
          {/if}
          {#if j.current}
            <span class="activity-current" title={j.current}>{j.current}</span>
          {/if}
        </div>
        <button class="danger" onclick={() => stopJob(j.id)}>
          {j.cancelled ? "stopping…" : "Stop"}
        </button>
      </div>
    {/each}
    {#each recent as j (j.id)}
      <div class="activity-row activity-done">
        <span class="hint">{jobLabel(j)}</span>
      </div>
    {/each}
  </div>
{/if}
