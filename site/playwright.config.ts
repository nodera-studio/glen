import { defineConfig, devices } from '@playwright/test';
// GLEN_E2E_PORT runs the suite on another port, so a second checkout can test beside one on 4321.
const port = process.env.GLEN_E2E_PORT ?? '4321', baseURL = `http://127.0.0.1:${port}`;
export default defineConfig({
 testDir: 'tests', fullyParallel: true, forbidOnly: !!process.env.CI, retries: process.env.CI ? 1 : 0,
 reporter: [['list'], ['html', { open: 'never' }]],
 use: { baseURL },
 projects: [{name:'chromium',use:{...devices['Desktop Chrome']}}],
 globalSetup: './scripts/e2e-server-identity.mjs',
 // --ignore-lock keeps astro preview in the foreground under an agent. Reuse is opt-in (GLEN_E2E_REUSE=1), and globalSetup refuses a server that is not serving this build.
 webServer: { command:`npx --no -- astro preview --host 127.0.0.1 --port ${port} --ignore-lock`,url:`${baseURL}/`,reuseExistingServer:process.env.GLEN_E2E_REUSE === '1',timeout:60_000 },
});
