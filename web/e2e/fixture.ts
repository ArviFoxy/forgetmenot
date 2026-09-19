// The store, the server and the hook events the Playwright run uses. Everything the
// tests see comes from here, so a run does not depend on any server that happens to
// be up and the screenshots are of a known store.

import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

/** The repository root, from this file's place in it. */
export const repoRoot = path.resolve(import.meta.dirname, '../..');

/** Cargo writes where it is told; in the container that is outside the repository. */
const targetDirectory = process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, 'target');
export const serverBinary = path.join(targetDirectory, 'release', 'forgetmenot');
export const exampleStore = path.join(repoRoot, 'examples', 'store');
export const webDist = path.join(repoRoot, 'web', 'dist');

/** Where the running fixture records what has to be torn down. */
export const stateFile = path.join(tmpdir(), 'forgetmenot-e2e-fixture.json');

export interface FixtureState {
  pid: number;
  directory: string;
  port: number;
}

export function makeFixtureDirectory(): string {
  return mkdtempSync(path.join(tmpdir(), 'forgetmenot-e2e-'));
}

/**
 * The commit the seeded store gets. The date is fixed so the commit id is the same
 * in every run, which is what lets the history page be compared to an image.
 */
export const seedCommit = {
  message: 'seed the example store',
  name: 'seed',
  email: 'seed@example.invalid',
  date: '2026-01-01T00:00:00+00:00',
};

export interface HookPlay {
  machine: string;
  /** A file under crates/forgetmenot-server/tests/fixtures/hooks. */
  fixture: string;
  contextTokens: number;
  /**
   * What the subagent the event comes from was asked to do. No hook event carries
   * it: the client reads it from the subagent's metadata file and sends it beside
   * the event, which is what is reproduced here.
   */
  task?: string;
}

/**
 * The events the fixture plays, in this order. They decide which memories count as
 * delivered and which contexts exist, so the contexts page and the dashboard show the
 * same rows in every run.
 */
export const hookSequence: HookPlay[] = [
  { machine: 'alpha', fixture: 'session_start', contextTokens: 0 },
  { machine: 'alpha', fixture: 'user_prompt_submit', contextTokens: 1200 },
  { machine: 'alpha', fixture: 'pre_tool_use_bash', contextTokens: 2400 },
  { machine: 'alpha', fixture: 'post_tool_use', contextTokens: 3600 },
  {
    machine: 'alpha',
    fixture: 'subagent_start',
    contextTokens: 3600,
    task: 'Survey the rocketry crate and list its public functions',
  },
  { machine: 'beta', fixture: 'session_start', contextTokens: 0 },
  { machine: 'beta', fixture: 'user_prompt_submit', contextTokens: 800 },
  { machine: 'alpha', fixture: 'stop', contextTokens: 4800 },
];

export const hookFixtureDirectory = path.join(
  repoRoot,
  'crates',
  'forgetmenot-server',
  'tests',
  'fixtures',
  'hooks',
);
