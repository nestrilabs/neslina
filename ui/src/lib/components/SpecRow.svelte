<script lang="ts">
	// A label on the left, its mono value on the right, a rule beneath: the
	// shape for every technical fact.
	let { label, value }: { label: string; value: string } = $props();
</script>

<div class="row">
	<span class="label">{label}</span>
	<span class="value">{value}</span>
</div>

<style>
	.row {
		align-items: baseline;
		border-bottom: 1px solid var(--color-border);
		display: flex;
		gap: 16px;
		justify-content: space-between;
		padding: 9px 0;
	}

	.label {
		color: var(--color-muted);
	}

	.value {
		color: var(--color-foreground);
		font-family: var(--font-mono);
		font-size: 0.93rem;
		overflow-wrap: anywhere;
		text-align: right;
	}
</style>
