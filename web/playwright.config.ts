import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'e2e',
  timeout: 30_000,
  use: { baseURL: process.env.FORGETMENOT_URL ?? 'http://127.0.0.1:7373' },
  reporter: [['list']],
});
