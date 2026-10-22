<script lang="ts">
	// Choosing the state disk and, optionally, the games disk. Nothing is
	// chosen for the owner, and erasing is confirmed by typing the names of
	// the disks that will be erased: there is no default to click through.
	import type { Disk } from '#lib/types/Disk.ts';
	import type { Disks } from '#lib/types/Disks.ts';
	import { gib } from '#lib/api.ts';
	import Button from './Button.svelte';

	let { disks, busy, onchoose }: { disks: Disk[]; busy: boolean; onchoose: (d: Disks) => void } = $props();

	let stateDisk = $state('');
	let gamesDisk = $state('');
	let typed = $state('');

	const usable = $derived(disks.filter((d) => !d.holds_medium));
	const chosen = $derived([stateDisk, gamesDisk].filter((n) => n !== ''));
	const confirmation = $derived(chosen.join(' '));
	const ready = $derived(stateDisk !== '' && stateDisk !== gamesDisk && typed.trim() === confirmation);

	function describe(d: Disk) {
		return [d.model, d.transport, d.partitions.length ? `${d.partitions.length} partitions` : 'empty']
			.filter(Boolean)
			.join(' · ');
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!ready) return;
		// Saved to setup.toml for later boots, where kernel names can change: a
		// serial when the disk has one.
		const pick = (name: string) => {
			const serial = disks.find((d) => d.name === name)?.serial;
			return serial ? { serial, wipe: true } : { name, wipe: true };
		};
		onchoose(gamesDisk ? { state: pick(stateDisk), games: pick(gamesDisk) } : { state: pick(stateDisk) });
	}
</script>

<form onsubmit={submit}>
	<table>
		<thead>
			<tr><th>Disk</th><th>Size</th><th>What it is</th><th>State</th><th>Games</th></tr>
		</thead>
		<tbody>
			{#each disks as d (d.name)}
				<tr class:medium={d.holds_medium}>
					<td class="mono">{d.name}</td>
					<td class="mono">{gib(d.size)}</td>
					<td>
						{#if d.holds_medium}
							the USB stick neslina runs from; never offered
						{:else}
							{describe(d)}{#if d.serial}<br /><span class="mono dim">{d.serial}</span>{/if}
						{/if}
					</td>
					<td>
						{#if !d.holds_medium}
							<input type="radio" name="state" value={d.name} bind:group={stateDisk} aria-label="{d.name} as the state disk" />
						{/if}
					</td>
					<td>
						{#if !d.holds_medium}
							<input type="radio" name="games" value={d.name} bind:group={gamesDisk} aria-label="{d.name} as the games disk" />
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>

	{#if usable.length === 0}
		<p class="error">There is no disk besides the USB stick. A host needs one internal disk.</p>
	{:else}
		<p class="hint">
			The state disk holds the host's build and its data; it is required. A games disk is optional; without
			one, games go on the state disk.
			{#if gamesDisk}<button type="button" class="link" onclick={() => (gamesDisk = '')}>No games disk</button>{/if}
		</p>
		{#if stateDisk && stateDisk === gamesDisk}
			<p class="error">The state and games disks must be different disks.</p>
		{/if}
		{#if chosen.length > 0 && stateDisk !== gamesDisk}
			<label class="confirm">
				<span>
					Everything on <strong>{chosen.join(' and ')}</strong> will be erased. To go ahead, type
					<code>{confirmation}</code>
				</span>
				<input type="text" bind:value={typed} autocomplete="off" spellcheck="false" />
			</label>
		{/if}
		<Button type="submit" variant="commit" disabled={!ready || busy}>Erase and install</Button>
	{/if}
</form>

<style>
	table {
		border-collapse: collapse;
		margin-bottom: 18px;
		width: 100%;
	}

	th {
		color: var(--color-gray-600);
		font-family: var(--font-mono);
		font-size: 11px;
		font-weight: 400;
		text-align: left;
		text-transform: uppercase;
	}

	th,
	td {
		border-bottom: 1px solid var(--color-border);
		padding: 9px 8px;
		vertical-align: top;
	}

	td:first-child,
	th:first-child {
		padding-left: 0;
	}

	.medium {
		color: var(--color-gray-600);
	}

	.mono {
		color: var(--color-foreground);
		font-family: var(--font-mono);
	}

	.dim {
		color: var(--color-gray-600);
		font-size: 12px;
	}

	input[type='radio'] {
		accent-color: var(--color-accent);
	}

	.hint {
		font-size: 14px;
	}

	.link {
		background: none;
		border: 0;
		color: var(--color-foreground);
		cursor: pointer;
		font: inherit;
		padding: 0;
		text-decoration: underline;
	}

	.confirm {
		display: grid;
		gap: 8px;
		margin-bottom: 18px;
	}

	@media (max-width: 560px) {
		th:nth-child(3),
		td:nth-child(3) {
			display: none;
		}
	}
</style>
