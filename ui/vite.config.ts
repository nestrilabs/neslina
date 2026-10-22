import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			// Static files, embedded in neslina-setup. Every path the server does not
			// have as a file gets index.html, so the app routes on the client.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		// `deno task dev` against a VM: its setup server is forwarded to 8443.
		proxy: { '/api': { target: 'https://127.0.0.1:8443', secure: false } }
	}
});
