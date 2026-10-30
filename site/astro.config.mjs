import { defineConfig } from 'astro/config';
import react from '@astrojs/react';
import tailwindcss from '@tailwindcss/vite';
// Site origin (deployed 2026-10-30, W7). Keep in sync with README.md's Website line.
// tests/readme.rs checks that the two agree; check-links.mjs reads the default as the site's origin.
export default defineConfig({ site: process.env.SITE_URL || 'https://glen.nodera.workers.dev', output: 'static', build: { inlineStylesheets: 'always' }, integrations: [react()], vite: { plugins: [tailwindcss()] }, devToolbar: { enabled: false } });
