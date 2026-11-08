<script lang="ts">
  // Report strip with an explicit dismiss button (field report #4: the
  // "already in the flow" line had no way to close it).
  let {
    text,
    tone = "",
    onDismiss,
  }: {
    text: string;
    tone?: "" | "error";
    onDismiss: () => void;
  } = $props();
</script>

<div class="report" class:error-strip={tone === "error"}>
  <span class="report-text">{text}</span>
  <button class="report-x" title="dismiss" onclick={onDismiss}>×</button>
</div>
