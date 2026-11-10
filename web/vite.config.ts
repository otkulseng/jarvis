import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  // `npm run dev` serves the page with hot reload and forwards API calls to
  // the running server.
  server: { proxy: { '/api': 'http://127.0.0.1:7878' } },
});
