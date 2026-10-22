<script lang="ts">
	import { onMount } from 'svelte';
	import { api, ApiError, gib, type State } from '#lib/api.ts';
	import type { Disks } from '#lib/types/Disks.ts';
	import Button from '#lib/components/Button.svelte';
	import DiskChoice from '#lib/components/DiskChoice.svelte';
	import Panel from '#lib/components/Panel.svelte';
	import SpecRow from '#lib/components/SpecRow.svelte';
	import Stages from '#lib/components/Stages.svelte';
	import Terminal from '#lib/components/Terminal.svelte';

	const STAGES = ['disks', 'copy bundle', 'native build', 'write slot', 'reboot'];
	const INSTALL_STAGE: Record<string, number> = { copy: 1, build: 2, place: 3, handover: 4 };

	let st: State | undefined = $state();
	let unauthorised = $state(false);
	let rebooting = $state(false);
	let afterReboot = $state(false);
	let error = $state('');
	let busy = $state(false);
	let log: string[] = $state([]);

	const host = $derived(st?.host);
	const job = $derived(st?.job);
	const stage = $derived(host?.install_stage);
	const installing = $derived(job?.state === 'running' || (stage != null && stage !== 'failed'));
	const failed = $derived(job?.state === 'failed' || stage === 'failed');
	const current = $derived(
		job?.state === 'running' && job.step === 'disks' ? 0 : stage ? (INSTALL_STAGE[stage] ?? 0) : 0
	);
	// The newest "building <component>" line of the build log, for the label under the bar.
	const building = $derived(
		log
			.toReversed()
			.find((l) => l.startsWith('building '))
			?.replace(/^building /, '')
	);

	async function refresh() {
		try {
			st = await api.state();
			unauthorised = false;
			if (rebooting) {
				rebooting = false;
				afterReboot = true;
			}
		} catch (e) {
			if (e instanceof ApiError && e.status === 401) {
				unauthorised = true;
				if (rebooting) {
					// Sessions live in memory, so the restarted host asks again.
					rebooting = false;
					afterReboot = true;
				}
				return;
			}
			// The server went away mid-install. The installer only stops answering
			// when it hands over and the host restarts into its build; the last
			// stages can pass between two polls, so any stage counts.
			if (installing || rebooting) {
				rebooting = true;
				error = '';
				return;
			}
			error = e instanceof Error ? e.message : String(e);
			return;
		}
		if (installing || failed) {
			log = (await api.log().catch(() => ({ lines: log }))).lines;
		}
	}

	async function act(action: () => Promise<unknown>) {
		error = '';
		busy = true;
		try {
			await action();
			await refresh();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	function onLogin(event: SubmitEvent) {
		event.preventDefault();
		const password = new FormData(event.currentTarget as HTMLFormElement).get('password');
		act(() => api.login(String(password ?? '')));
	}

	function onPassword(event: SubmitEvent) {
		event.preventDefault();
		const form = new FormData(event.currentTarget as HTMLFormElement);
		const [a, b] = [String(form.get('password') ?? ''), String(form.get('again') ?? '')];
		if (a !== b) {
			error = 'The two passwords differ.';
			return;
		}
		act(() => api.setPassword(a));
	}

	const install = (disks: Disks) => act(() => api.assignDisks(disks));

	onMount(() => {
		refresh();
		const timer = setInterval(refresh, 3000);
		return () => clearInterval(timer);
	});
</script>

<header>
	<p class="micro" aria-hidden="true">neslina / setup</p>
	<h1 class="lockup">{host?.hostname ?? 'neslina'}</h1>
	{#if host}
		<p class="sub">
			Running <code>{host.slot}</code>, {host.version}. Reachable as <code>{host.hostname}.local</code>.
		</p>
	{/if}
</header>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if rebooting}
	<Panel label="restart">
		<h2>Restarting into its own build</h2>
		<p>
			The host built its slot and is starting it now. This page comes back by itself once the new slot is up;
			you will be asked to log in again.
		</p>
	</Panel>
{:else if unauthorised}
	<Panel label="login">
		<h2>Log in</h2>
		{#if afterReboot}
			<p>The host restarted into its own build. Log in again with your password.</p>
		{:else}
			<p>
				The first time, use the password in <code>neslina/setup.toml</code> on the USB stick. You will set your
				own right after.
			</p>
		{/if}
		<form class="stack" onsubmit={onLogin}>
			<label class="field">
				<span>Password</span>
				<input type="password" name="password" autocomplete="current-password" />
			</label>
			<div><Button type="submit" variant="primary" disabled={busy}>Log in</Button></div>
		</form>
	</Panel>
{:else if !st || !host}
	<p>Connecting…</p>
{:else if st.must_change_password}
	<Panel label="password">
		<h2>Set your own password</h2>
		<p>
			At least 12 characters. It replaces the one on the USB stick, which is then removed; the stick keeps only
			a hash of yours.
		</p>
		<form class="stack" onsubmit={onPassword}>
			<label class="field">
				<span>New password</span>
				<input type="password" name="password" autocomplete="new-password" minlength="12" />
			</label>
			<label class="field">
				<span>The same again</span>
				<input type="password" name="again" autocomplete="new-password" minlength="12" />
			</label>
			<div><Button type="submit" variant="primary" disabled={busy}>Save password</Button></div>
		</form>
	</Panel>
{:else}
	<Panel label="machine">
		<h2>This machine</h2>
		<SpecRow label="Processor" value={host.cpu} />
		<SpecRow label="Threads" value={String(host.threads)} />
		<SpecRow label="Memory" value={gib(host.memory_bytes)} />
		<SpecRow label="State disk" value={host.state_disk ? 'set up' : 'not yet'} />
	</Panel>

	{#if host.slot !== 'rescue'}
		<Panel label="done">
			<h2>Set up</h2>
			<p>
				This host runs <strong>slot {host.slot}</strong>, built on this machine for its own CPU. Setup is done.
			</p>
		</Panel>
	{:else if installing || failed}
		<Panel label="install">
			<h2>{failed ? 'The install stopped' : 'Installing'}</h2>
			<Stages stages={STAGES} {current} {failed} />
			{#if failed}
				<p class="error">
					{job?.state === 'failed' ? `The ${job.step} step failed: ${job.reason}.` : 'The native build failed.'}
					The end of the log is below. Nothing has been written to the USB stick.
				</p>
			{:else if current === 2}
				<p>
					Building neslina for this machine's CPU{#if building}: <code>{building}</code>{/if}. The kernel takes
					the longest.
				</p>
			{/if}
			<Terminal lines={log} label="build log" />
		</Panel>
	{:else if job?.state === 'finished'}
		<Panel label="install">
			<h2>Nothing was installed</h2>
			<p>
				The disks are ready, but there is no install bundle in <code>neslina/install/</code> on the USB stick.
				Put one there and restart the host.
			</p>
		</Panel>
	{:else}
		<Panel label="disks">
			<h2>Choose disks</h2>
			<DiskChoice disks={host.disks} {busy} onchoose={install} />
		</Panel>
	{/if}

	<p class="logout"><Button onclick={() => act(api.logout)}>Log out</Button></p>
{/if}

<style>
	header {
		margin-bottom: 36px;
	}

	.lockup {
		color: var(--color-foreground);
		font-family: var(--font-crt);
		font-size: clamp(28px, 6vw, 44px);
		font-weight: 400;
		letter-spacing: 0;
		margin: 6px 0 10px;
		text-transform: uppercase;
	}

	.sub {
		font-size: 14px;
	}

	:global(.panel) + :global(.panel) {
		margin-top: 24px;
	}

	.stack {
		display: grid;
		gap: 12px;
		max-width: 360px;
	}

	.field {
		display: grid;
		gap: 6px;
	}

	.field span {
		color: var(--color-gray-600);
		font-family: var(--font-mono);
		font-size: 11px;
		text-transform: uppercase;
	}

	.logout {
		margin-top: 32px;
	}
</style>
