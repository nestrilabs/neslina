<script lang="ts">
	// A region marked by four detached corner ticks rather than a border, with
	// an optional micro-label in its frame. The label is chrome: the heading
	// inside carries the same meaning.
	import type { Snippet } from 'svelte';

	let { label, children }: { label?: string; children: Snippet } = $props();
</script>

<section class="panel">
	<span class="tick tl" aria-hidden="true"></span>
	<span class="tick tr" aria-hidden="true"></span>
	<span class="tick bl" aria-hidden="true"></span>
	<span class="tick br" aria-hidden="true"></span>
	{#if label}<span class="micro corner" aria-hidden="true">{label}</span>{/if}
	{@render children()}
</section>

<style>
	.panel {
		background: var(--color-background-100);
		padding: 28px;
		position: relative;
	}

	.tick {
		border-color: var(--color-gray-alpha-500);
		border-style: solid;
		height: 10px;
		position: absolute;
		width: 10px;
	}

	.tl {
		border-width: 1px 0 0 1px;
		left: -1px;
		top: -1px;
	}

	.tr {
		border-width: 1px 1px 0 0;
		right: -1px;
		top: -1px;
	}

	.bl {
		border-width: 0 0 1px 1px;
		bottom: -1px;
		left: -1px;
	}

	.br {
		border-width: 0 1px 1px 0;
		bottom: -1px;
		right: -1px;
	}

	.corner {
		position: absolute;
		right: 14px;
		top: 8px;
	}

	@media (max-width: 560px) {
		.panel {
			padding: 20px 16px;
		}
	}
</style>
