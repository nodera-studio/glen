import { defineConfig } from 'astro/config';
import react from '@astrojs/react';
import tailwindcss from '@tailwindcss/vite';
export default defineConfig({ site: process.env.SITE_URL || undefined, output: 'static', build: { inlineStylesheets: 'always' }, integrations: [react()], vite: { plugins: [tailwindcss()] }, devToolbar: { enabled: false } });
