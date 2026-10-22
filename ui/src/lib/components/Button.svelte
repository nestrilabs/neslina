<script lang="ts">
	// One button, as on nestri.io: square, Departure Mono uppercased in CSS.
	// `primary` is the inverted fill, `secondary` the outline, and `commit` the
	// brand orange, kept for the one action on a screen that changes the machine.
	import type { Snippet } from 'svelte';

	let {
		variant = 'secondary',
		type = 'button',
		disabled = false,
		onclick,
		children
	}: {
		variant?: 'primary' | 'secondary' | 'commit';
		type?: 'button' | 'submit';
		disabled?: boolean;
		onclick?: () => void;
		children: Snippet;
	} = $props();
</script>

<button class="btn btn-{variant}" {type} {disabled} {onclick}>{@render children()}</button>

<style>
	.btn {
		align-items: center;
		border: 0;
		border-radius: 0;
		cursor: pointer;
		display: inline-flex;
		font-family: var(--font-crt);
		font-size: 15px;
		gap: 10px;
		line-height: 1;
		padding: 10px 18px;
		text-transform: uppercase;
		transition: background-color 0.2s ease;
		user-select: none;
		white-space: nowrap;
	}

	.btn:disabled {
		cursor: not-allowed;
		opacity: 0.4;
	}

	.btn-primary {
		background: var(--color-gray-1000);
		color: var(--color-background-100);
	}

	.btn-primary:hover:not(:disabled) {
		background: #ccc;
	}

	.btn-secondary {
		background: var(--color-background-100);
		box-shadow: 0 0 0 1px var(--color-gray-alpha-400);
		color: var(--color-gray-1000);
	}

	.btn-secondary:hover:not(:disabled) {
		background: var(--color-gray-alpha-200);
	}

	/* Orange is for fills with dark text, never for small orange text. */
	.btn-commit {
		background: var(--color-brand);
		color: var(--color-background-200);
	}

	.btn-commit:hover:not(:disabled) {
		background: var(--color-brand-hover);
	}
</style>
