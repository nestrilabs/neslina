// The setup server's API. Its types come from the Rust structs (ts-rs), so a
// change on the server side breaks this build instead of the page.
import type { Disks } from './types/Disks';
import type { Host } from './types/Host';
import type { Job } from './types/Job';

export type State = { host: Host; job: Job; must_change_password: boolean };

export class ApiError extends Error {
	constructor(
		message: string,
		readonly status: number
	) {
		super(message);
	}
}

async function call<T>(path: string, body?: unknown): Promise<T> {
	const init: RequestInit =
		body === undefined
			? {}
			: { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) };
	const response = await fetch(path, init);
	const data: unknown = await response.json().catch(() => ({}));
	if (!response.ok) {
		const message =
			typeof data === 'object' && data !== null && 'error' in data && typeof data.error === 'string'
				? data.error
				: response.statusText;
		throw new ApiError(message, response.status);
	}
	return data as T;
}

export const api = {
	state: () => call<State>('/api/state'),
	login: (password: string) => call<{ must_change_password: boolean }>('/api/login', { password }),
	logout: () => call<object>('/api/logout', {}),
	setPassword: (password: string) => call<object>('/api/password', { password }),
	assignDisks: (disks: Disks) => call<object>('/api/disks', disks),
	log: () => call<{ lines: string[] }>('/api/log')
};

export const gib = (bytes: number) => `${Math.round(bytes / 2 ** 30)} GiB`;
