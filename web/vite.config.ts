/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';

// `pnpm dev` proxies the API to a running hue-jack (or scripts/mock-server.mjs).
const api = process.env.HUEJACK_API ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [preact()],
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    proxy: {
      '/api': api,
      '/ws': { target: api.replace(/^http/, 'ws'), ws: true },
    },
  },
  test: { environment: 'jsdom' },
});
