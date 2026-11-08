<script lang="ts">
  // Live activity strip: what is running right now, which item it is on, and a
  // Stop button — no more frozen toolbar while something imports in the background.
  import { activity, jobLabel, stopJob } from "../lib/activity.svelte";

  const live = $derived(activity.jobs.filter((j) => !j.finished));
  const recent = $derived(
    activity.jobs.filter((j) => j.finished).slice(-3).reverse(),
  );
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
