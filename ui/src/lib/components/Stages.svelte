<script lang="ts">
	// The install's steps as one flat, segmented bar, with each label inside its
	// segment. Done steps fill grey, the current one fills blue, a failed one
	// is red. Fullness, not a hue, says a step is done.
	let { stages, current, failed = false }: { stages: string[]; current: number; failed?: boolean } = $props();
</script>

<ol class="stages">
	{#each stages as stage, i (stage)}
		<li
			class:done={i < current}
			class:current={i === current && !failed}
			class:failed={i === current && failed}
			aria-current={i === current ? 'step' : undefined}>
			{stage}
		</li>
	{/each}
</ol>

<style>
	.stages {
		display: grid;
		gap: 2px;
		grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
		list-style: none;
		margin: 0 0 20px;
		padding: 0;
	}

	li {
		background: var(--color-gray-alpha-200);
		color: var(--color-gray-600);
		font-family: var(--font-mono);
		font-size: 12px;
		padding: 8px 10px;
		text-transform: lowercase;
	}

	.done {
		background: var(--color-gray-alpha-500);
		color: var(--color-foreground);
	}

	.current {
		background: color-mix(in hsl, var(--color-accent) 35%, transparent);
		color: var(--color-foreground);
	}

	.failed {
		background: color-mix(in hsl, var(--color-down) 30%, transparent);
		color: var(--color-foreground);
	}
</style>
