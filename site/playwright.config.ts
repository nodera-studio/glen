import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
 testDir: 'tests', fullyParallel: true, forbidOnly: !!process.env.CI, retries: process.env.CI ? 1 : 0,
 reporter: [['list'], ['html', { open: 'never' }]],
 use: { baseURL: 'http://127.0.0.1:4321' },
 projects: [{name:'chromium',use:{...devices['Desktop Chrome']}}],
 globalSetup: './scripts/e2e-server-identity.mjs',
 // --ignore-lock keeps astro preview in the foreground under an agent. Reuse is opt-in (GLEN_E2E_REUSE=1), and globalSetup refuses a server that is not serving this build.
 webServer: { command:'npm run preview -- --ignore-lock',url:'http://127.0.0.1:4321/',reuseExistingServer:process.env.GLEN_E2E_REUSE === '1',timeout:60_000 },
});
