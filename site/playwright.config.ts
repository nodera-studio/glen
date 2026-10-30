import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
 testDir: 'tests', fullyParallel: true, forbidOnly: !!process.env.CI, retries: process.env.CI ? 1 : 0,
 reporter: [['list'], ['html', { open: 'never' }]],
 use: { baseURL: 'http://127.0.0.1:4321' },
 projects: [{name:'chromium',use:{...devices['Desktop Chrome']}}],
 webServer: { command:'npm run preview',url:'http://127.0.0.1:4321/',reuseExistingServer:!process.env.CI,timeout:60_000 },
});
