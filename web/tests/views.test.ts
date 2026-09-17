// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from 'vitest';
import type {
  ContextPrompt,
  ContextRow,
  MemoryDoc,
  MemoryStatsRow,
  ScopeRow,
} from '../src/api/types';
import type { TreeNode } from '../src/model/tree';

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

/** The message the scope on the server carries. */
const scopeMessage = 'A widget part number is never renumbered.';

/** The context tokens after which the scope on the server forgets itself. */
const scopeForgetTokens = 50000;

/** The scopes the index lists: one with a file, one without. */
const scopeRows: ScopeRow[] = [
  { id: 'global', kind: 'global', name: null, file: null },
  {
    id: 'widgets',
    kind: 'file',
    name: null,
    file: {
      id: 'widgets',
      message: scopeMessage,
      implies: [],
      triggers: [],
      forget: { tokens_since_trigger: scopeForgetTokens },
      version: 'v',
    },
  },
];

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
 * The prompt the server renders for one context, with a text long enough that its
 * size is reported in kilobytes rather than as a count of bytes.
 */
const promptText = `[forgetmenot] context alpha/session-1\n${'the rule that binds this session.\n'.repeat(60)}`;

const contextPrompt: ContextPrompt = {
  key: 'alpha/session-1',
  name: 'Rebuild the thermocouple rig',
  mode: 'all',
  text: promptText,
  bytes: promptText.length,
  tokens_estimate: Math.ceil(promptText.length / 4),
};

/**
 * What the mocked API answers with, which a test sets before it renders. Hoisted
 * because the mock factory below runs before anything else in this file.
 */
const answers = vi.hoisted(() => ({
  contexts: [] as unknown[],
  contextPrompt: null as unknown,
  scopeWrites: [] as { scope_message: string | null; forget: { tokens_since_trigger: number } | null }[],
}));

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
    scopeIndex: () => Promise.resolve(scopeRows),
    scope: () =>
      Promise.resolve({
        id: 'widgets',
        message: null,
        implies: [],
        triggers: [],
        forget: null,
        version: 'v',
      }),
    putScope: (
      _id: string,
      request: { scope_message: string | null; forget: { tokens_since_trigger: number } | null },
    ) => {
      answers.scopeWrites.push(request);
      return Promise.resolve({ kind: 'written' });
    },
    deleteScope: () => Promise.resolve({ kind: 'written' }),
    createScope: () => Promise.resolve({ kind: 'written' }),
    scopeHistory: () => Promise.resolve([]),
    testTriggers: () => Promise.resolve({ fired: [] }),
    validatePattern: () => Promise.resolve({ ok: true }),
    settings: () => Promise.resolve({ settings: {}, version: null, schema: [] }),
    putSetting: () => Promise.resolve({ kind: 'written' }),
    machineIndex: () => Promise.resolve(['alpha', 'beta']),
    contexts: () => Promise.resolve(answers.contexts),
    contextPrompt: () => Promise.resolve(answers.contextPrompt),
    review: () => Promise.resolve({ errors: [], global_only_critical: [] }),
    memoryStats: () => Promise.resolve([memoryRow]),
    triggerStats: () => Promise.resolve([]),
    scopeStats: () => Promise.resolve([]),
    denyStats: () => Promise.resolve([]),
    latencyStats: () => Promise.resolve([]),
    sessionStats: () => Promise.resolve([]),
  },
}));

const { formatBytes } = await import('../src/model/units');
const { paths, resolve } = await import('../src/routes');
const { menuItemsFor } = await import('../src/components/fmn-sidebar');
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
  answers.contextPrompt = contextPrompt;
  answers.scopeWrites = [];
});

/** One live context, with everything the contexts page reads. */
function context(over: Partial<ContextRow> & { key: string }): ContextRow {
  return {
    name: '',
    title: null,
    first_prompt: null,
    parent: null,
    task: null,
    agent_type: null,
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
    paths.contextPrompt('alpha/session-1/agent-7f3a'),
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

// The source of these three expectations is the decision about what the contexts
// table shows: the columns Name, Scopes, Id, Machine, Delivered, Last seen, in
// that order, with each subagent directly under the session it runs in and its
// name indented, and the rows in the order the server sends them, which is most
// recently seen first (issue #21).

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

/** A session with no subagent, keyed ahead of `session` and seen before it. */
const otherSession = context({
  key: 'alpha/session-0',
  name: 'Sort the fastener bins',
  last_seen: '2026-01-02T03:04:30+00:00',
});

test('a session whose subagent was seen last is listed under a session seen earlier', async () => {
  // The order the server sends: most recently seen first, which puts the
  // subagent ahead of both sessions and leaves its own session last.
  await renderContexts([subagent, otherSession, session]);

  const rows = [...document.querySelectorAll('table.data tbody tr')];
  expect(rows.map((row) => cellsOf(row)[0])).toEqual([
    session.name,
    subagent.name,
    otherSession.name,
  ]);
  expect(rows[1]?.querySelector('td')?.classList.contains('nested')).toBe(true);
});

// The source of these two expectations is this ticket: the prompt page shows the
// text as it would arrive, in a monospace block, with the size it costs above it in
// the unit a reader of the page uses; and the name in the contexts table is the way
// to that page.

test('the prompt page shows the size in raw bytes, or hides the text the context would be given', async () => {
  const page = document.createElement('fmn-context-prompt-view') as HTMLElement & {
    contextKey: string;
    updateComplete?: Promise<unknown>;
  };
  page.contextKey = contextPrompt.key;
  document.body.append(page);
  await settle(page);

  const size = page.querySelector('.prompt-size')?.textContent ?? '';
  expect(size).toContain(formatBytes(contextPrompt.bytes));
  expect(size).toContain(String(contextPrompt.tokens_estimate));
  expect(page.querySelector('pre.prompt')?.textContent).toBe(contextPrompt.text);
});

test('the prompt page shows an empty text as a block of nothing instead of saying so', async () => {
  answers.contextPrompt = { ...contextPrompt, mode: 'due', text: '', bytes: 0, tokens_estimate: 0 };
  const page = document.createElement('fmn-context-prompt-view') as HTMLElement & {
    contextKey: string;
    updateComplete?: Promise<unknown>;
  };
  page.contextKey = contextPrompt.key;
  document.body.append(page);
  await settle(page);

  expect(page.querySelector('pre.prompt')).toBeNull();
  expect(page.textContent).toContain('Nothing due');
});

test('the name in the contexts table opens nothing, so the prompt page is unreachable', async () => {
  const nameless = context({ key: 'beta/session-9', name: '' });
  await renderContexts([session, nameless]);

  const links = [...document.querySelectorAll('table.data tbody td.context-name a')];
  expect(links.map((link) => link.getAttribute('href'))).toEqual([
    paths.contextPrompt(session.key),
    paths.contextPrompt(nameless.key),
  ]);
  // A context the server could not name is opened by its id, so no row is a link
  // with nothing to click.
  expect(links[1]?.textContent?.trim()).toBe('session-9');
});

// The source of this expectation is issue #7: a scope file may carry a message, so
// the page that edits a scope has to show the one on the server and send the one the
// reader typed.

test('the scope page hides the message the scope carries, or writes back another text', async () => {
  const page = document.createElement('fmn-scope-page') as HTMLElement & {
    scopeId: string;
    updateComplete?: Promise<unknown>;
  };
  page.scopeId = 'widgets';
  document.body.append(page);
  await settle(page);

  expect(page.textContent).toContain(scopeMessage);

  page.querySelector<HTMLElement>('sl-icon-button[label="Edit Message"]')?.click();
  await settle(page);
  const field = page.querySelector('sl-textarea');
  expect(field, 'the Message field must be editable').not.toBeNull();
  expect(field?.getAttribute('value')).toBe(scopeMessage);

  const edited = 'Renumbering a shipped part number is a release blocker.';
  field!.value = edited;
  field!.dispatchEvent(new CustomEvent('sl-input'));
  await settle(page);
  const bar = page.querySelector('fmn-commit-bar');
  expect(bar, 'an edited message must offer to be saved').not.toBeNull();
  bar?.dispatchEvent(
    new CustomEvent('fmn-message-change', { detail: { message: 'reword it' }, bubbles: true }),
  );
  bar?.dispatchEvent(new CustomEvent('fmn-save', { bubbles: true }));
  await settle(page);

  expect(answers.scopeWrites.map((request) => request.scope_message)).toEqual([edited]);
});

// The source of this expectation is this ticket: a scope file may declare when it
// forgets itself, so the page that edits a scope has to show the count on the server
// and send the one the reader typed.

test('the scope page shows the forget count the scope carries and writes back another', async () => {
  const page = document.createElement('fmn-scope-page') as HTMLElement & {
    scopeId: string;
    updateComplete?: Promise<unknown>;
  };
  page.scopeId = 'widgets';
  document.body.append(page);
  await settle(page);

  expect(page.textContent).toContain(String(scopeForgetTokens));

  page.querySelector<HTMLElement>('sl-icon-button[label="Edit Forget after"]')?.click();
  await settle(page);
  const field = page.querySelector('sl-input[type="number"]');
  expect(field, 'the Forget after field must be editable').not.toBeNull();
  expect(field?.getAttribute('value')).toBe(String(scopeForgetTokens));

  field!.setAttribute('value', '1200');
  (field as HTMLInputElement).value = '1200';
  field!.dispatchEvent(new CustomEvent('sl-input'));
  await settle(page);
  const bar = page.querySelector('fmn-commit-bar');
  expect(bar, 'an edited forget count must offer to be saved').not.toBeNull();
  bar?.dispatchEvent(
    new CustomEvent('fmn-message-change', { detail: { message: 'forget it sooner' }, bubbles: true }),
  );
  bar?.dispatchEvent(new CustomEvent('fmn-save', { bubbles: true }));
  await settle(page);

  expect(answers.scopeWrites.map((request) => request.forget)).toEqual([
    { tokens_since_trigger: 1200 },
  ]);
});

// The source of these two expectations is what the tree menu can act on: the file is
// the only thing a delete removes, and a memory made from a session scope belongs to
// that session.

/** The tree node of one scope row, as the tree draws it. */
function scopeNode(row: ScopeRow): TreeNode {
  return {
    kind: 'scope',
    key: `scope:${row.id}`,
    label: row.id,
    scopeId: row.id,
    scope: row,
    children: [],
    count: 0,
  };
}

function menuLabels(node: TreeNode): string[] {
  return menuItemsFor(node).map((item) => item.label);
}

test('the tree menu offers to delete a scope that has no file to delete', () => {
  const session: ScopeRow = {
    id: 'session:alpha/session-1',
    kind: 'session',
    name: 'Rebuild the thermocouple rig',
    file: null,
  };
  expect(menuLabels(scopeNode(session))).not.toContain('Delete scope');
  expect(menuLabels(scopeNode(scopeRows[0]!))).not.toContain('Delete scope');
  expect(menuLabels(scopeNode(scopeRows[1]!))).toContain('Delete scope');
});

test('the tree menu makes a memory in this scope out of a session', () => {
  const session: ScopeRow = {
    id: 'session:alpha/session-1',
    kind: 'session',
    name: null,
    file: null,
  };
  expect(menuLabels(scopeNode(session))).toContain('New memory in this session');
  expect(menuLabels(scopeNode(scopeRows[1]!))).toContain('New memory in this scope');
});
