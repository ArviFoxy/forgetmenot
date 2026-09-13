// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from 'vitest';
import type { ContextRow, MemoryDoc, MemoryStatsRow } from '../src/api/types';

// The source of these expectations is what the pages promise: every address the app
// links to has an element to show it, the statistics keep index-line and full
// deliveries in separate columns, and a rejected write shows both versions.

const serverDoc: MemoryDoc = {
  id: 'widget-naming',
  name: 'widget-naming',
  title: 'Widget part numbers are immutable',
  description: 'a widget part number is never reused',
  kind: 'critical',
  scopes: ['widgets'],
  source: 'user',
  metadata: {},
  created: null,
  modified: null,
  author: 'wiki',
  body: '# Widget part numbers are immutable\n\nthe text that is on the server\n',
  version: 'b'.repeat(40),
  links: [],
  backlinks: [],
  last_commit: null,
};

const memoryRow: MemoryStatsRow = {
  memory: 'widget-naming',
  shown_index: 1,
  shown_full_new: 2,
  shown_full_changed: 3,
  shown_full_stale: 4,
  fetched_full: 5,
  retracted: 6,
  last_shown: '2026-01-02T03:04:05+00:00',
};

/**
 * What the mocked API answers with, which a test sets before it renders. Hoisted
 * because the mock factory below runs before anything else in this file.
 */
const answers = vi.hoisted(() => ({ contexts: [] as unknown[] }));

vi.mock('../src/api/client', () => ({
  RequestFailed: class RequestFailed extends Error {
    constructor(
      readonly method: string,
      readonly url: string,
      readonly status: number,
      readonly bodyText: string,
    ) {
      super(`${method} ${url} failed with ${status}`);
    }
  },
  MissingCommitMessage: class MissingCommitMessage extends Error {},
  api: {
    memoryIndex: () => Promise.resolve([]),
    memory: () => Promise.resolve(serverDoc),
    putMemory: () => Promise.resolve({ kind: 'written' }),
    createMemory: () => Promise.resolve({ kind: 'written' }),
    deleteMemory: () => Promise.resolve({ kind: 'written' }),
    memoryHistory: () => Promise.resolve([]),
    memoryHistoryEntry: () => Promise.resolve({ commit: {}, content: '', diff: '' }),
    scopeIndex: () => Promise.resolve([]),
    scope: () =>
      Promise.resolve({ id: 'widgets', implies: [], triggers: [], version: 'v' }),
    putScope: () => Promise.resolve({ kind: 'written' }),
    deleteScope: () => Promise.resolve({ kind: 'written' }),
    createScope: () => Promise.resolve({ kind: 'written' }),
    scopeHistory: () => Promise.resolve([]),
    testTriggers: () => Promise.resolve({ fired: [] }),
    validatePattern: () => Promise.resolve({ ok: true }),
    settings: () => Promise.resolve({ settings: {}, version: null, schema: [] }),
    putSetting: () => Promise.resolve({ kind: 'written' }),
    machineIndex: () => Promise.resolve(['alpha', 'beta']),
    contexts: () => Promise.resolve(answers.contexts),
    review: () => Promise.resolve({ errors: [], global_only_critical: [] }),
    memoryStats: () => Promise.resolve([memoryRow]),
    triggerStats: () => Promise.resolve([]),
    scopeStats: () => Promise.resolve([]),
    denyStats: () => Promise.resolve([]),
    latencyStats: () => Promise.resolve([]),
    sessionStats: () => Promise.resolve([]),
  },
}));

const { paths, resolve } = await import('../src/routes');
await import('../src/components/fmn-app');

/** Waits for the element and everything it loads to settle. */
async function settle(element: HTMLElement & { updateComplete?: Promise<unknown> }): Promise<void> {
  for (let round = 0; round < 20; round += 1) {
    await element.updateComplete;
    await new Promise((done) => setTimeout(done, 0));
  }
}

beforeEach(() => {
  document.body.innerHTML = '';
  answers.contexts = [];
});

/** One live context, with everything the contexts page reads. */
function context(over: Partial<ContextRow> & { key: string }): ContextRow {
  return {
    name: '',
    title: null,
    first_prompt: null,
    parent: null,
    task: null,
    active_scopes: ['global'],
    delivered_count: 1,
    last_seen: '2026-01-02T03:04:05+00:00',
    ...over,
  };
}

const session = context({
  key: 'alpha/session-1',
  name: 'Rebuild the thermocouple rig',
  title: 'Rebuild the thermocouple rig',
  delivered_count: 3,
});

const subagent = context({
  key: 'alpha/session-1/agent-7f3a',
  name: 'Survey the rocketry crate',
  parent: 'alpha/session-1',
  task: 'Survey the rocketry crate',
  last_seen: '2026-01-02T03:05:05+00:00',
});

/** The contexts page, rendered over `rows`. */
async function renderContexts(rows: ContextRow[]): Promise<void> {
  answers.contexts = rows;
  const element = document.createElement('fmn-contexts-view');
  document.body.append(element);
  await settle(element);
}

function cellsOf(row: Element | undefined): string[] {
  return [...(row?.querySelectorAll('td') ?? [])].map((cell) => cell.textContent?.trim() ?? '');
}

test('an address the app links to has no element registered to show it', () => {
  const addresses = [
    paths.home(),
    paths.memory('sessions/alpha/session-1/notes'),
    paths.memoryHistory('sessions/alpha/session-1/notes'),
    paths.memoryCommit('sessions/alpha/session-1/notes', 'a'.repeat(40)),
    paths.memoryNew(),
    paths.memoryNew('widgets'),
    paths.scopeNew(),
    paths.scope('session:alpha/session-1'),
    paths.contexts(),
    paths.stats(),
    paths.settings(),
    '/no/such/address',
  ];
  for (const address of addresses) {
    const view = resolve(address);
    expect(customElements.get(view.tag), `${address} -> ${view.tag}`).toBeTypeOf('function');
  }
});

test('the statistics add the delivery counts together instead of showing each', async () => {
  const element = document.createElement('fmn-stats-view');
  document.body.append(element);
  await settle(element);

  const cells = [...document.querySelectorAll('table.stats td')].map((cell) =>
    cell.textContent?.trim(),
  );
  for (const count of ['1', '2', '3', '4', '5', '6']) {
    expect(cells).toContain(count);
  }
  expect(cells).toContain('2026-01-02T03:04:05+00:00');
});

// The source of these two expectations is the decision about what the contexts
// table shows: the columns Name, Scopes, Id, Machine, Delivered, Last seen, in
// that order, with each subagent directly under the session it runs in and its
// name indented.

test('the contexts table names its columns in another order, or lists a context without its name', async () => {
  await renderContexts([session]);

  const headers = [...document.querySelectorAll('table.data thead th')].map((cell) =>
    cell.textContent?.trim(),
  );
  expect(headers).toEqual(['Name', 'Scopes', 'Id', 'Machine', 'Delivered', 'Last seen']);
  const [row] = [...document.querySelectorAll('table.data tbody tr')];
  expect(cellsOf(row)[0]).toBe(session.name);
  // The key is split over the two columns that carry its parts.
  expect(cellsOf(row)).toContain('session-1');
  expect(cellsOf(row)).toContain('alpha');
});

test('a subagent is listed in the order it arrived, or level with the session it runs in', async () => {
  // In the other order, so that a page that lists them as they arrive fails.
  await renderContexts([subagent, session]);

  const rows = [...document.querySelectorAll('table.data tbody tr')];
  expect(rows.map((row) => cellsOf(row)[0])).toEqual([session.name, subagent.name]);
  expect(rows[1]?.querySelector('td')?.classList.contains('nested')).toBe(true);
  expect(rows[0]?.querySelector('td')?.classList.contains('nested')).toBe(false);
});
