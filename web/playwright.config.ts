import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'e2e',
  timeout: 30_000,
  use: { baseURL: process.env.FORGETMENOT_URL ?? 'http://127.0.0.1:7373' },
  // The screenshots are compared tightly: a shifted layout changes only the pixels
  // the text covers, which on a large viewport is a small share of the image.
  expect: { toHaveScreenshot: { maxDiffPixelRatio: 0.001, animations: 'disabled' } },
  reporter: [['list']],
});
