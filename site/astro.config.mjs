import { defineConfig } from 'astro/config';
import react from '@astrojs/react';
import tailwindcss from '@tailwindcss/vite';
// SITE_URL placeholder: after the first deploy (W7 Step 10), replace `undefined` below with the
// quoted https://glen.<subdomain>.workers.dev URL and put the same URL on README.md's SITE_URL line.
// tests/readme.rs checks that the two agree; check-links.mjs reads the default as the site's origin.
export default defineConfig({ site: process.env.SITE_URL || undefined, output: 'static', build: { inlineStylesheets: 'always' }, integrations: [react()], vite: { plugins: [tailwindcss()] }, devToolbar: { enabled: false } });
