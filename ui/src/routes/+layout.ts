// A single-page app: no server rendering and nothing prerendered. The server
// answers every path with index.html, and the page runs entirely in the
// browser against neslina-setup's JSON API.
export const ssr = false;
export const prerender = false;
