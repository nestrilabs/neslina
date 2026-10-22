<script lang="ts">
	// A mono log block that keeps its end in view as lines arrive.
	let { lines, label }: { lines: string[]; label: string } = $props();
	let pre: HTMLPreElement | undefined = $state();

	$effect(() => {
		void lines.length;
		if (pre) pre.scrollTop = pre.scrollHeight;
	});
</script>

<figure>
	<figcaption class="micro">{label}</figcaption>
	<pre bind:this={pre}>{lines.length ? lines.join('\n') : 'waiting for output'}</pre>
</figure>

<style>
	figure {
		margin: 0;
	}

	figcaption {
		margin-bottom: 6px;
		opacity: 1;
	}

	pre {
		background: var(--color-background-200);
		box-shadow: 0 0 0 1px var(--color-gray-alpha-300);
		color: var(--color-gray-1000);
		font-family: var(--font-mono);
		font-size: 12px;
		line-height: 1.6;
		margin: 0;
		max-height: 340px;
		overflow: auto;
		padding: 14px 16px;
		white-space: pre-wrap;
		word-break: break-all;
	}
</style>
