// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from 'vitest';
import type { MemoryDoc, MemoryStatsRow } from '../src/api/types';

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
  created: null,
  modified: null,
  author: 'wiki',
  archived: false,
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

const putMemory = vi.fn();

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
    putMemory: (...args: unknown[]) => putMemory(...args) as unknown,
    createMemory: () => Promise.resolve({ kind: 'written' }),
    archiveMemory: () => Promise.resolve({ kind: 'written' }),
    memoryHistory: () => Promise.resolve([]),
    memoryHistoryEntry: () => Promise.resolve({ commit: {}, content: '', diff: '' }),
    scopeIndex: () => Promise.resolve([]),
    scope: () =>
      Promise.resolve({ id: 'widgets', type: 'project', implies: [], triggers: [], version: 'v' }),
    putScope: () => Promise.resolve({ kind: 'written' }),
    createScope: () => Promise.resolve({ kind: 'written' }),
    scopeHistory: () => Promise.resolve([]),
    testTriggers: () => Promise.resolve({ fired: [] }),
    contexts: () => Promise.resolve([]),
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
  putMemory.mockReset();
});

test('an address the app links to has no element registered to show it', () => {
  const addresses = [
    paths.home(),
    paths.memory('sessions/alpha/session-1/notes'),
    paths.memoryEdit('sessions/alpha/session-1/notes'),
    paths.memoryHistory('sessions/alpha/session-1/notes'),
    paths.memoryCommit('sessions/alpha/session-1/notes', 'a'.repeat(40)),
    paths.memoryNew(),
    paths.scope('session:alpha/session-1'),
    paths.scopeEdit('widgets'),
    paths.contexts(),
    paths.review(),
    paths.stats(),
    paths.triggerTest(),
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

test('a write the server rejects as stale loses one of the two versions', async () => {
  const editorText = '# Widget part numbers are immutable\n\nthe text the editor holds\n';
  putMemory.mockResolvedValue({ kind: 'conflict', conflict: { current: serverDoc } });

  const element = document.createElement('fmn-memory-page');
  Object.assign(element, { memoryId: 'widget-naming', mode: 'edit', oid: '' });
  document.body.append(element);
  await settle(element);

  const editor = element.querySelector('fmn-source-editor') as HTMLElement & { value: string };
  editor.dispatchEvent(
    new CustomEvent('fmn-source-change', { detail: { value: editorText }, bubbles: true }),
  );
  const message = element.querySelector('#commit-message') as HTMLInputElement;
  message.value = 'widen the rule';
  message.dispatchEvent(new Event('input', { bubbles: true }));
  await settle(element);

  const save = [...element.querySelectorAll('button')].find((button) =>
    button.textContent?.includes('Save'),
  );
  save?.click();
  await settle(element);

  const shown = element.textContent ?? '';
  expect(shown).toContain('the text the editor holds');
  expect(shown).toContain('the text that is on the server');
});
