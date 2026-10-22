<script lang="ts">
	import '#lib/styles.css';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<svelte:head>
	<title>neslina setup</title>
</svelte:head>

<div class="column">
	<span class="cross top-left" aria-hidden="true"></span>
	<span class="cross top-right" aria-hidden="true"></span>
	{@render children()}
</div>

<style>
	/* The grid: one dashed column with crosses where its rules meet the top. */
	.column {
		border-left: 1px dashed var(--color-border);
		border-right: 1px dashed var(--color-border);
		margin: 0 auto;
		max-width: 760px;
		min-height: 100vh;
		padding: 48px 40px 80px;
		position: relative;
	}

	.cross {
		height: 21px;
		position: absolute;
		top: 30px;
		width: 21px;
	}

	.cross::before,
	.cross::after {
		background: var(--color-gray-alpha-500);
		content: '';
		position: absolute;
	}

	.cross::before {
		height: 1px;
		left: 0;
		top: 10px;
		width: 21px;
	}

	.cross::after {
		height: 21px;
		left: 10px;
		top: 0;
		width: 1px;
	}

	.top-left {
		left: -11px;
	}

	.top-right {
		right: -11px;
	}

	@media (max-width: 560px) {
		.column {
			padding: 32px 16px 64px;
		}
	}
</style>
