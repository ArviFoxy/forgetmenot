// Stops the fixture server and removes its store, so a run leaves nothing behind.

import { existsSync, readFileSync, rmSync } from 'node:fs';
import { stateFile, type FixtureState } from './fixture';

export default async function globalTeardown(): Promise<void> {
  if (!existsSync(stateFile)) return;
  const state = JSON.parse(readFileSync(stateFile, 'utf8')) as FixtureState;
  rmSync(stateFile, { force: true });

  if (state.pid > 0) {
    try {
      process.kill(state.pid, 'SIGTERM');
    } catch {
      // It is already gone.
    }
    // The server writes its context snapshot on SIGTERM; give it a moment.
    for (let waited = 0; waited < 50; waited += 1) {
      await new Promise((done) => setTimeout(done, 100));
      try {
        process.kill(state.pid, 0);
      } catch {
        break;
      }
    }
  }
  rmSync(state.directory, { recursive: true, force: true });
}
