import { execFileSync } from 'node:child_process';
import { defineConfig } from '@playwright/test';

/**
 * A port nothing is listening on. The address has to be known before the tests
 * start, which is why it is taken here rather than in the global setup.
 */
function freePort(): number {
  const found = execFileSync(
    process.execPath,
    [
      '-e',
      "const s=require('node:net').createServer();s.listen(0,'127.0.0.1',()=>{process.stdout.write(String(s.address().port));s.close();});",
    ],
    { encoding: 'utf8' },
  );
  return Number.parseInt(found.trim(), 10);
}

// With FORGETMENOT_URL the tests run against that server; without it the global
// setup seeds a store and starts the release binary on this port. The port is put in
// the environment because this config is read again in every worker, and all of them
// have to agree on the address.
function fixturePort(): number {
  if (process.env.FORGETMENOT_URL !== undefined) return 0;
  const already = Number.parseInt(process.env.FMN_FIXTURE_PORT ?? '', 10);
  const port = Number.isInteger(already) && already > 0 ? already : freePort();
  process.env.FMN_FIXTURE_PORT = String(port);
  return port;
}

const port = fixturePort();

export default defineConfig({
  testDir: 'e2e',
  timeout: 30_000,
  globalSetup: './e2e/global-setup.ts',
  globalTeardown: './e2e/global-teardown.ts',
  use: { baseURL: process.env.FORGETMENOT_URL ?? `http://127.0.0.1:${port}` },
  // The appearance run goes first, against the store as the fixture seeded it; the
  // flows write to that store, so they run after and cannot change what the images
  // were compared against.
  projects: [
    { name: 'appearance', testMatch: /visual\.spec\.ts$/ },
    {
      name: 'flows',
      testMatch: /edit-flow\.spec\.ts$/,
      // Only the fixture run needs the order: it is the writes that have to come
      // after the images. Against an outside server the two are independent.
      ...(process.env.FORGETMENOT_URL === undefined ? { dependencies: ['appearance'] } : {}),
    },
  ],
  // The screenshots are compared tightly: a shifted layout changes only the pixels
  // the text covers, which on a large viewport is a small share of the image.
  expect: { toHaveScreenshot: { maxDiffPixelRatio: 0.001, animations: 'disabled' } },
  reporter: [['list']],
});
