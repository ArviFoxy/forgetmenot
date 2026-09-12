// Brings up everything the tests need: a copy of the example store in a temporary
// git repository, the release server on the port the config picked, and a fixed
// sequence of hook events. FORGETMENOT_URL skips all of it and uses that server.

import { execFileSync, spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import {
  exampleStore,
  hookFixtureDirectory,
  hookSequence,
  makeFixtureDirectory,
  repoRoot,
  seedCommit,
  serverBinary,
  stateFile,
  webDist,
  type FixtureState,
} from './fixture';

function run(command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv = {}): void {
  execFileSync(command, args, { cwd, stdio: 'inherit', env: { ...process.env, ...env } });
}

/** A store of its own, committed, so writes in the tests cannot reach the repository. */
function seedStore(directory: string): string {
  const store = path.join(directory, 'store');
  cpSync(exampleStore, store, { recursive: true });
  run('git', ['init', '-q', '-b', 'main'], store);
  run('git', ['add', '-A'], store);
  // Both dates are fixed, because the commit id is made from them: with the clock in
  // there the history page would show a different id in every run.
  run(
    'git',
    [
      '-c',
      `user.name=${seedCommit.name}`,
      '-c',
      `user.email=${seedCommit.email}`,
      'commit',
      '-q',
      '-m',
      seedCommit.message,
    ],
    store,
    { GIT_AUTHOR_DATE: seedCommit.date, GIT_COMMITTER_DATE: seedCommit.date },
  );
  return store;
}

async function waitForHealth(base: string): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${base}/api/health`);
      if (response.ok) return;
    } catch {
      // The server is not listening yet.
    }
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`the fixture server did not answer on ${base} within 30 s`);
}

async function playHooks(base: string): Promise<void> {
  for (const play of hookSequence) {
    const payload: unknown = JSON.parse(
      readFileSync(path.join(hookFixtureDirectory, `${play.fixture}.json`), 'utf8'),
    );
    const response = await fetch(`${base}/hook`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        machine: play.machine,
        context_tokens: play.contextTokens,
        hook: payload,
      }),
    });
    if (!response.ok) {
      throw new Error(`the hook ${play.fixture} on ${play.machine} answered ${response.status}`);
    }
  }
}

/**
 * The server records deliveries and trigger fires behind the request, so a page read
 * straight after the hooks can show a partial count. This waits until two reads in a
 * row report the same thing, which is what makes the statistics page comparable to
 * an image.
 */
async function settleStatistics(base: string): Promise<void> {
  const read = async (): Promise<string> => {
    const paths = ['/api/stats/memories', '/api/stats/triggers', '/api/stats/scopes', '/api/contexts'];
    const answers = await Promise.all(
      paths.map(async (path) => (await fetch(`${base}${path}`)).text()),
    );
    return answers.join('|');
  };
  let previous = await read();
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    await new Promise((done) => setTimeout(done, 250));
    const current = await read();
    if (current === previous) return;
    previous = current;
  }
  throw new Error('the statistics did not settle within 15 s');
}

export default async function globalSetup(): Promise<void> {
  if (process.env.FORGETMENOT_URL !== undefined) {
    // An outside server was named; the tests use it as it is.
    return;
  }

  const port = Number(process.env.FMN_FIXTURE_PORT);
  if (!Number.isInteger(port) || port <= 0) {
    throw new Error('the Playwright config did not pass a port for the fixture server');
  }

  if (!existsSync(serverBinary)) {
    run('cargo', ['build', '--release', '-p', 'forgetmenot-server'], repoRoot);
  }
  if (!existsSync(path.join(webDist, 'index.html'))) {
    run('npm', ['run', 'build'], path.join(repoRoot, 'web'));
  }

  const directory = makeFixtureDirectory();
  const store = seedStore(directory);
  mkdirSync(path.join(directory, 'run'), { recursive: true });
  const log = openSync(path.join(directory, 'server.log'), 'a');

  const server = spawn(
    serverBinary,
    [
      'serve',
      '--store',
      store,
      '--listen',
      `127.0.0.1:${port}`,
      '--state-path',
      path.join(directory, 'run', 'contexts.json'),
      '--stats-path',
      path.join(directory, 'run', 'stats.sqlite'),
      '--web-dist',
      webDist,
    ],
    { cwd: repoRoot, stdio: ['ignore', log, log], detached: false },
  );
  server.unref();

  const state: FixtureState = { pid: server.pid ?? 0, directory, port };
  writeFileSync(stateFile, JSON.stringify(state));

  const base = `http://127.0.0.1:${port}`;
  await waitForHealth(base);
  await playHooks(base);
  await settleStatistics(base);
  process.stdout.write(`fixture store at ${directory}, server on ${base}\n`);
}
