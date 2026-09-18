// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from 'vitest';
import type {
  CommitFiles,
  ContextPrompt,
  ContextRow,
  MemoryDoc,
  MemoryStatsRow,
  ScopeRow,
  ScopeStatsRow,
  Series,
  SessionStatsRow,
  StoreHistory,
  Summary,
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
  shrunk: 6,
  retracted: 7,
  chars: 3500,
  tokens: 1000,
  most_under: 'widgets',
  last_shown: '2026-01-02T03:04:05+00:00',
};

/** A second scope, cheaper than `widgets` and named ahead of it in the alphabet. */
const cheaperScopeStatsRow: ScopeStatsRow = {
  scope_id: 'rocketry',
  activations: 1,
  forgettings: 0,
  chars: 3500,
  deliveries: 1,
  live_contexts: 1,
  tokens: 1000,
  tokens_per_delivery: 1000,
  memories: 1,
};

const scopeStatsRow: ScopeStatsRow = {
  scope_id: 'widgets',
  activations: 2,
  forgettings: 1,
  chars: 42000,
  deliveries: 4,
  live_contexts: 1,
  tokens: 12000,
  tokens_per_delivery: 3000,
  memories: 5,
};

const sessionStatsRow: SessionStatsRow = {
  session_key: 'alpha/session-1',
  bytes_full: 100,
  bytes_index: 20,
  chars: 3500,
  tokens: 1000,
  last_context_tokens: 4800,
};

/** The summary the page turns into its five cards. */
const summary: Summary = {
  windows: [
    { name: '5m', chars: 10, tokens: 3, events: 1, held: 0, forgettings: 0 },
    { name: '1h', chars: 4200, tokens: 1200, events: 8, held: 2, forgettings: 1 },
    { name: '1d', chars: 119000, tokens: 34000, events: 40, held: 7, forgettings: 2 },
    { name: '7d', chars: 5250000, tokens: 1500000, events: 300, held: 19, forgettings: 9 },
  ],
  live_contexts: 3,
};

const series: Series = {
  bucket: 'hour',
  points: [
    { t: '2026-01-02T02:00:00Z', tokens: 800, deliveries: 2 },
    { t: '2026-01-02T03:00:00Z', tokens: 400, deliveries: 1 },
  ],
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
  tokens: Math.ceil(promptText.length / 3.5),
};

/**
 * What the mocked API answers with, which a test sets before it renders. Hoisted
 * because the mock factory below runs before anything else in this file.
 */
const answers = vi.hoisted(() => ({
  /** Whether the statistics answers carry rows, so an empty range can be shown. */
  statsRows: true,
  contexts: [] as unknown[],
  contextPrompt: null as unknown,
  scopeWrites: [] as { scope_message: string | null; forget: { tokens_since_trigger: number } | null }[],
  storeHistory: null as unknown,
  storeCommit: null as unknown,
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
    storeHistory: () => Promise.resolve(answers.storeHistory),
    storeCommit: () => Promise.resolve(answers.storeCommit),
    testTriggers: () => Promise.resolve({ fired: [] }),
    validatePattern: () => Promise.resolve({ ok: true }),
    settings: () => Promise.resolve({ settings: {}, version: null, schema: [] }),
    putSetting: () => Promise.resolve({ kind: 'written' }),
    machineIndex: () => Promise.resolve(['alpha', 'beta']),
    contexts: () => Promise.resolve(answers.contexts),
    contextPrompt: () => Promise.resolve(answers.contextPrompt),
    review: () => Promise.resolve({ errors: [], global_only_critical: [] }),
    memoryStats: () => Promise.resolve(answers.statsRows ? [memoryRow] : []),
    triggerStats: () => Promise.resolve([]),
    scopeStats: () =>
      Promise.resolve(answers.statsRows ? [cheaperScopeStatsRow, scopeStatsRow] : []),
    denyStats: () => Promise.resolve([]),
    latencyStats: () => Promise.resolve([]),
    sessionStats: () => Promise.resolve(answers.statsRows ? [sessionStatsRow] : []),
    summaryStats: () => Promise.resolve(summary),
    seriesStats: () => Promise.resolve(answers.statsRows ? series : { bucket: 'hour', points: [] }),
    sessionSeries: () => Promise.resolve([]),
  },
}));

const { formatBytes } = await import('../src/model/units');
const { paths, resolve } = await import('../src/routes');
const { menuItemsFor } = await import('../src/components/fmn-sidebar');
await import('../src/components/fmn-app');

// jsdom has no media queries and the tables listen for the widths at which their
// columns change. The window here is wide enough for every column, so what this
// file asserts is the table with all of them drawn.
window.matchMedia = ((media: string) => ({
  media,
  matches: false,
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
})) as unknown as typeof window.matchMedia;

/** Waits for the element and everything it loads to settle. */
async function settle(element: HTMLElement & { updateComplete?: Promise<unknown> }): Promise<void> {
  for (let round = 0; round < 20; round += 1) {
    await element.updateComplete;
    await new Promise((done) => setTimeout(done, 0));
  }
}

beforeEach(() => {
  document.body.innerHTML = '';
  answers.statsRows = true;
  window.history.replaceState(null, '', '/stats');
  window.localStorage.clear();
  answers.contexts = [];
  answers.contextPrompt = contextPrompt;
  answers.scopeWrites = [];
  answers.storeHistory = storeHistory;
  answers.storeCommit = storeCommit;
});

/** The store's commits, newest first, as the history list reads them. */
const storeHistory: StoreHistory = {
  commits: [
    {
      oid: 'c'.repeat(40),
      time: '2026-01-02T03:04:05+00:00',
      author: 'wiki',
      title: 'widen the widget rule',
    },
    {
      oid: 'd'.repeat(40),
      time: '2026-01-01T00:00:00+00:00',
      author: 'seed',
      title: 'seed the example store',
    },
  ],
  next_before: null,
};

/**
 * One commit that changed a memory, a scope and a file that holds neither, each
 * with a diff of its own so a page that shows one file's diff under another fails.
 */
const storeCommit: CommitFiles = {
  commit: storeHistory.commits[0]!,
  files: [
    {
      path: 'memories/widget-naming.md',
      status: 'modified',
      diff: '@@ -1 +1 @@\n-a part number is never reused\n+a widget part number is never reused\n',
      memory_id: 'widget-naming',
      scope_id: null,
    },
    {
      path: 'scopes/widgets.yaml',
      status: 'modified',
      diff: '@@ -1 +1 @@\n-triggers: []\n+triggers: [widget]\n',
      memory_id: null,
      scope_id: 'widgets',
    },
    {
      path: 'config.yml',
      status: 'modified',
      diff: '@@ -1 +1 @@\n-announce_empty_scopes: false\n+announce_empty_scopes: true\n',
      memory_id: null,
      scope_id: null,
    },
  ],
};

/** The page each file of the commit above opens, `null` when it holds no document. */
const commitFileTargets: (string | null)[] = [
  paths.memory('widget-naming'),
  paths.scope('widgets'),
  null,
];

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
    paths.history(),
    paths.historyCommit('c'.repeat(40)),
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

// The source of these four expectations is what the statistics page promises: five
// figures across the top, a window with nothing in it said to be empty rather than
// drawn as an empty table, a click on a scope row narrowing every section to that
// scope through the address, and every memory row a way to that memory's page.

/** The statistics page, rendered and settled. */
async function renderStats(): Promise<HTMLElement> {
  const element = document.createElement('fmn-stats-view');
  document.body.append(element);
  await settle(element);
  return element;
}

test('the summary shows some other number of figures, or reports raw characters', async () => {
  const element = await renderStats();

  const cards = [...element.querySelectorAll('.summary-card')];
  expect(cards).toHaveLength(5);
  const values = cards.map((card) => card.querySelector('.summary-value')?.textContent?.trim());
  // The tokens of the hour, the day and the week, then the calls held today and
  // the contexts working now.
  expect(values).toEqual(['1.2k', '34k', '1.5M', '7', '3']);
});

test('a range with nothing in it is drawn as an empty chart and empty tables', async () => {
  answers.statsRows = false;
  const element = await renderStats();

  const empties = [...element.querySelectorAll('.empty')].map((node) => node.textContent?.trim());
  expect(empties.length).toBeGreaterThanOrEqual(4);
  for (const text of empties) expect(text).toBe('Nothing in this range');
  expect(element.querySelector('fmn-token-series')).toBeNull();
});

test('a click on a scope row leaves the address alone, so the other sections keep their window', async () => {
  const element = await renderStats();

  const row = [...element.querySelectorAll('.stats-scopes tr.data-row')].find(
    (each) => each.querySelector('td.row-id')?.textContent?.trim() === scopeStatsRow.scope_id,
  );
  expect(row, 'the scope table must have a row to click').not.toBeUndefined();
  (row as HTMLElement).click();
  await settle(element);

  expect(window.location.search).toBe('?scope=widgets');
});

test('the scope table opens in the order the server sent, or hides which way it is sorted', async () => {
  // The server sends the rows by scope id; the table is a report of what things
  // cost, so it opens on the costliest.
  const element = await renderStats();

  const scopes = element.querySelector('.stats-scopes');
  const first = [...(scopes?.querySelectorAll('tr.data-row') ?? [])].map(
    (row) => row.querySelector('td.row-id')?.textContent?.trim(),
  );
  expect(first).toEqual([scopeStatsRow.scope_id, cheaperScopeStatsRow.scope_id]);
  const tokens = [...(scopes?.querySelectorAll('thead th') ?? [])].find(
    (cell) => cell.textContent?.trim() === 'Tokens',
  );
  expect(tokens?.getAttribute('aria-sort')).toBe('descending');
});

test('a click on Tokens sorts it the way it already is, so the order does not change', async () => {
  const element = await renderStats();
  const scopes = element.querySelector('.stats-scopes');
  const tokens = [...(scopes?.querySelectorAll('thead th') ?? [])].find(
    (cell) => cell.textContent?.trim() === 'Tokens',
  );

  (tokens?.querySelector('button.sort') as HTMLElement).click();
  await settle(element);

  expect(tokens?.getAttribute('aria-sort')).toBe('ascending');
  const order = [...(scopes?.querySelectorAll('tr.data-row') ?? [])].map(
    (row) => row.querySelector('td.row-id')?.textContent?.trim(),
  );
  expect(order).toEqual([cheaperScopeStatsRow.scope_id, scopeStatsRow.scope_id]);
});

/** A session the registry holds but nobody ever named. */
const namelessSession = context({ key: 'alpha/session-4' });

test('the session select labels an option by its key, or offers a nameless session or a subagent', async () => {
  // The source of this expectation is what the select is for: choosing a
  // session by what it was doing. A subagent is not a session, and a context
  // with no name has nothing to read as, so neither is on offer.
  answers.contexts = [subagent, namelessSession, session];
  const element = await renderStats();

  const [sessions] = [...element.querySelectorAll('.series-filter')];
  const options = [...(sessions?.querySelectorAll('sl-option') ?? [])].map((option) => [
    option.getAttribute('value'),
    option.textContent?.trim(),
  ]);
  expect(options).toEqual([
    ['', 'All sessions'],
    [session.key, session.name],
  ]);
});

test('a memory row has no way to the memory it names', async () => {
  const element = await renderStats();

  const link = element.querySelector('.stats-memories tr.data-row a');
  expect(link?.getAttribute('href')).toBe(paths.memory(memoryRow.memory));
  expect(link?.textContent?.trim()).toBe(memoryRow.memory);
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
  expect(size).toContain(String(contextPrompt.tokens));
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

// The source of these expectations is this ticket: the store's history is the list
// of its commits, each row opening a page that shows every file that commit changed
// with that file's diff and a way to the file's own page.

test('the store history lists a commit with no way to the page of that commit', async () => {
  const page = document.createElement('fmn-history-view') as HTMLElement & {
    mode: string;
    updateComplete?: Promise<unknown>;
  };
  page.mode = 'list';
  document.body.append(page);
  await settle(page);

  const rows = [...page.querySelectorAll('table.data tbody tr')];
  expect(rows).toHaveLength(storeHistory.commits.length);
  expect(rows.map((row) => row.querySelector('a')?.getAttribute('href'))).toEqual(
    storeHistory.commits.map((commit) => paths.historyCommit(commit.oid)),
  );
  // The author and the time of a commit are on its row, beside its title.
  expect(cellsOf(rows[0])).toContain(storeHistory.commits[0]!.author);
  expect(cellsOf(rows[0])).toContain(storeHistory.commits[0]!.time);
  expect(cellsOf(rows[0])).toContain(storeHistory.commits[0]!.title);
});

test('the page of one commit folds its files into one diff, or leaves a file unreachable', async () => {
  const page = document.createElement('fmn-history-view') as HTMLElement & {
    mode: string;
    oid: string;
    updateComplete?: Promise<unknown>;
  };
  page.mode = 'commit';
  page.oid = storeCommit.commit.oid;
  document.body.append(page);
  await settle(page);

  const sections = [...page.querySelectorAll('.commit-file')];
  expect(sections).toHaveLength(storeCommit.files.length);
  for (const [index, file] of storeCommit.files.entries()) {
    const section = sections[index]!;
    const added = file.diff.split('\n').find((line) => line.startsWith('+')) ?? '';
    expect(section.textContent, file.path).toContain(file.path);
    expect(section.querySelector('fmn-diff')?.textContent, file.path).toContain(added);
    expect(section.querySelector('a')?.getAttribute('href') ?? null, file.path).toBe(
      commitFileTargets[index] ?? null,
    );
  }
});
