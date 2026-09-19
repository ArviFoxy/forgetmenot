import { expect, test } from 'vitest';
import type {
  ContextRow,
  MemoryStatsRow,
  ScopeStatsRow,
  SeriesPoint,
  SessionEventPoint,
  SessionStatsRow,
  Summary,
} from '../src/api/types';
import {
  defaultStatsFilter,
  hiddenColumns,
  measuredSizes,
  memoryColumns,
  memorySort,
  scopeColumns,
  scopeSort,
  sessionColumns,
  sessionOptions,
  sessionPoints,
  sessionRows,
  sessionSort,
  statsAddress,
  statsFilterFromSearch,
  statsQueryOf,
  statsSearch,
  statsWindow,
  summaryHeld,
  summaryTokens,
  tokenPoints,
  visibleColumns,
  type StatsFilter,
  type TableColumn,
  type TableSort,
} from '../src/model/stats';

// The source of these expectations is what the dashboard promises: the range
// names a window ending now, the address carries the range and the two filters so a
// reload and a shared link show the same thing, a chart reads instants and figures
// rather than text, and a column sorts by the figure it shows.

const now = new Date('2026-09-17T12:00:00.000Z');

test('a range measures its window from somewhere other than now, or gets the wrong length', () => {
  expect(statsWindow('24h', now)).toEqual({
    from: '2026-09-16T12:00:00.000Z',
    to: '2026-09-17T12:00:00.000Z',
  });
  expect(statsWindow('7d', now)).toEqual({
    from: '2026-09-10T12:00:00.000Z',
    to: '2026-09-17T12:00:00.000Z',
  });
  expect(statsWindow('30d', now)).toEqual({
    from: '2026-08-18T12:00:00.000Z',
    to: '2026-09-17T12:00:00.000Z',
  });
});

test('the whole log is asked for with a window, which would cut it off at one end', () => {
  expect(statsWindow('all', now)).toEqual({});
  expect(statsQueryOf({ range: 'all', session: '', scope: '' }, now)).toEqual({});
});

test('an empty session or scope is sent as a filter, which narrows the log to nothing', () => {
  expect(statsQueryOf({ range: 'all', session: '', scope: '' }, now)).toEqual({});
  expect(statsQueryOf({ range: 'all', session: 'alpha/session-1', scope: 'widgets' }, now)).toEqual({
    session: 'alpha/session-1',
    scope: 'widgets',
  });
});

test('a filter read back from the address is not the one written to it', () => {
  const filters: StatsFilter[] = [
    { range: '24h', session: '', scope: '' },
    { range: '7d', session: '', scope: 'widgets' },
    { range: '30d', session: 'alpha/session-1', scope: '' },
    { range: 'all', session: 'alpha/session-1/agent-7f3a', scope: 'session:alpha/session-1' },
  ];
  for (const filter of filters) {
    expect(statsFilterFromSearch(statsSearch(filter)), JSON.stringify(filter)).toEqual(filter);
  }
});

test('the address carries what the page would show anyway, so a plain link is not the default view', () => {
  expect(statsSearch(defaultStatsFilter)).toBe('');
  expect(statsAddress(defaultStatsFilter)).toBe('/dashboard');
  expect(statsAddress({ range: '7d', session: '', scope: 'widgets' })).toBe(
    '/dashboard?range=7d&scope=widgets',
  );
});

test('an address naming a range nobody offers is taken as that range instead of the default', () => {
  expect(statsFilterFromSearch('?range=12h').range).toBe('24h');
  expect(statsFilterFromSearch('').range).toBe('24h');
  expect(statsFilterFromSearch('?scope=widgets')).toEqual({
    range: '24h',
    session: '',
    scope: 'widgets',
  });
});

const seriesPoints: SeriesPoint[] = [
  { t: '2026-09-17T10:00:00Z', tokens: 1200, deliveries: 3 },
  { t: '2026-09-17T11:00:00Z', tokens: 340, deliveries: 1 },
];

test('the series keeps its bucket start as text, so the chart draws one band per bucket', () => {
  const points = tokenPoints(seriesPoints);
  expect(points.map((point) => point.at instanceof Date)).toEqual([true, true]);
  expect(points.map((point) => point.at.toISOString())).toEqual([
    '2026-09-17T10:00:00.000Z',
    '2026-09-17T11:00:00.000Z',
  ]);
  expect(points.map((point) => point.tokens)).toEqual([1200, 340]);
});

const events: SessionEventPoint[] = [
  { t: '2026-09-17T10:00:00Z', event: 'session_start', context_tokens: 0, tokens: 900 },
  { t: '2026-09-17T10:01:00Z', event: 'pre_tool_use', context_tokens: null, tokens: 40 },
  { t: '2026-09-17T10:02:00Z', event: 'stop', context_tokens: 4800, tokens: 0 },
];

test('an event that reported no context size is drawn as a size of zero', () => {
  const points = sessionPoints(events);
  expect(points.map((point) => point.contextTokens)).toEqual([0, null, 4800]);
  // The line is drawn through the events that carry a measurement, so a gap is a
  // gap rather than a dip to zero.
  expect(measuredSizes(points).map((point) => point.contextTokens)).toEqual([0, 4800]);
  expect(measuredSizes(points).map((point) => point.at.toISOString())).toEqual([
    '2026-09-17T10:00:00.000Z',
    '2026-09-17T10:02:00.000Z',
  ]);
});

const summary: Summary = {
  windows: [
    { name: '5m', chars: 10, tokens: 3, events: 1, held: 0, forgettings: 0 },
    { name: '1h', chars: 3500, tokens: 1000, events: 8, held: 2, forgettings: 1 },
    { name: '1d', chars: 35000, tokens: 10000, events: 40, held: 7, forgettings: 2 },
  ],
  live_contexts: 3,
};

test('a window the answer does not carry reads as the figure of another window', () => {
  expect(summaryTokens(summary, '1h')).toBe(1000);
  expect(summaryTokens(summary, '7d')).toBe(0);
  expect(summaryHeld(summary, '1d')).toBe(7);
  expect(summaryHeld(summary, '7d')).toBe(0);
});

const scopeRow: ScopeStatsRow = {
  scope_id: 'widgets',
  activations: 2,
  forgettings: 1,
  chars: 42000,
  deliveries: 4,
  live_contexts: 1,
  tokens: 12000,
  tokens_per_delivery: 3000.4,
  memories: 5,
};

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
  last_shown: null,
};

function column<Row>(columns: TableColumn<Row>[], id: string): TableColumn<Row> {
  const found = columns.find((each) => each.id === id);
  if (found === undefined) throw new Error(`no column ${id}`);
  return found;
}

test('a formatted figure sorts and filters by its text, so 9 comes after 10000', () => {
  const tokens = column(scopeColumns, 'tokens');
  expect(tokens.value(scopeRow)).toBe(12000);
  expect(tokens.text?.(scopeRow)).toBe('12k');
  const perDelivery = column(scopeColumns, 'per-delivery');
  expect(perDelivery.value(scopeRow)).toBe(3000.4);
  expect(perDelivery.text?.(scopeRow)).toBe('3000');
});

test('a missing time or scope is shown as the word null rather than as an empty cell', () => {
  expect(column(memoryColumns, 'last-shown').value(memoryRow)).toBe('');
  expect(column(memoryColumns, 'most-under').value({ ...memoryRow, most_under: null })).toBe('');
  expect(column(memoryColumns, 'most-under').value(memoryRow)).toBe('widgets');
});

test('a column leads somewhere other than the page of the thing it names', () => {
  expect(column(scopeColumns, 'scope').link?.(scopeRow)).toBe('/scopes/widgets');
  expect(column(memoryColumns, 'memory').link?.(memoryRow)).toBe('/memories/widget-naming');
  expect(
    column(sessionColumns, 'session').link?.({
      session_key: 'alpha/session-1',
      bytes_full: 0,
      bytes_index: 0,
      chars: 0,
      tokens: 0,
      last_context_tokens: null,
      last_seen: '',
    }),
  ).toBe('/contexts/alpha/session-1/prompt');
});

const sessionStats: SessionStatsRow[] = [
  {
    session_key: 'alpha/session-1',
    bytes_full: 100,
    bytes_index: 20,
    chars: 3500,
    tokens: 1000,
    last_context_tokens: 4800,
  },
  {
    session_key: 'beta/session-1',
    bytes_full: 10,
    bytes_index: 0,
    chars: 70,
    tokens: 20,
    last_context_tokens: null,
  },
];

test('a session the registry no longer holds takes another session row time as its own', () => {
  const rows = sessionRows(
    sessionStats,
    new Map([['alpha/session-1', '2026-09-17T11:59:00+00:00']]),
  );
  expect(rows.map((row) => row.last_seen)).toEqual(['2026-09-17T11:59:00+00:00', '']);
  // A context size nobody reported is left blank rather than shown as no tokens.
  const size = column(sessionColumns, 'context');
  expect(size.text?.(rows[0]!)).toBe('4.8k');
  expect(size.text?.(rows[1]!)).toBe('');
});

// The source of the next three expectations is what the tables promise at a size:
// on a phone every table shows what names the row and what it cost, on a tablet
// the figures a reader compares rows by as well, and on a laptop all of them, with
// nothing dropped on the way; and every table opens on its largest cost. The
// widths are the ones the recorded screenshots are taken at.

/** The three tables of the dashboard, each with what it opens on. */
const tables: { name: string; columns: TableColumn<never>[]; sort: TableSort }[] = [
  { name: 'scopes', columns: scopeColumns, sort: scopeSort },
  { name: 'memories', columns: memoryColumns, sort: memorySort },
  { name: 'sessions', columns: sessionColumns, sort: sessionSort },
];

/** The ids of the columns a table shows at `width`, in the order it draws them. */
function shownAt<Row>(columns: TableColumn<Row>[], width: number): string[] {
  return visibleColumns(columns, width).map((column) => column.id);
}

test('a table drawn on a phone shows a column the width cannot hold, or hides one it can', () => {
  expect(shownAt(scopeColumns, 390)).toEqual(['scope', 'tokens']);
  expect(shownAt(memoryColumns, 390)).toEqual(['memory', 'tokens']);
  expect(shownAt(sessionColumns, 390)).toEqual(['session', 'tokens']);

  expect(shownAt(scopeColumns, 820)).toEqual(['scope', 'tokens', 'deliveries', 'activations']);
  expect(shownAt(memoryColumns, 820)).toEqual(['memory', 'tokens', 'fetched']);
  expect(shownAt(sessionColumns, 820)).toEqual(['session', 'tokens', 'context']);

  for (const table of tables) {
    expect(shownAt(table.columns, 1440), table.name).toEqual(
      table.columns.map((column) => column.id),
    );
  }
});

test('a column the width leaves out is in neither list, so its figure is nowhere to be read', () => {
  for (const table of tables) {
    for (const width of [390, 820, 1440]) {
      const reachable = [
        ...shownAt(table.columns, width),
        ...hiddenColumns(table.columns, width).map((column) => column.id),
      ];
      expect(reachable.sort(), `${table.name} at ${width}px`).toEqual(
        table.columns.map((column) => column.id).sort(),
      );
    }
  }
});

test('a table opens on a column other than the cost, or on the smallest figure first', () => {
  for (const table of tables) {
    expect(table.sort, table.name).toEqual({ id: 'tokens', desc: true });
    expect(
      table.columns.find((column) => column.id === table.sort.id)?.numeric,
      `${table.name} must sort by a column it has`,
    ).toBe(true);
  }
});

/** One context as `/api/contexts` answers it. */
function contextRow(over: Partial<ContextRow> & { key: string }): ContextRow {
  return {
    name: '',
    title: null,
    first_prompt: null,
    parent: null,
    task: null,
    agent_type: null,
    active_scopes: [],
    delivered_count: 0,
    last_seen: '2026-01-02T03:04:05+00:00',
    ...over,
  };
}

test('the session in the address is dropped from the options, so the filter it names reads as unset', () => {
  // The registry holds one named session; the address names another it has
  // forgotten. Dropping the second would leave the select showing no value
  // while the page went on reading the log for that session.
  const held = contextRow({ key: 'alpha/session-1', name: 'Rebuild the rig' });

  expect(sessionOptions([held], 'alpha/session-9')).toEqual([
    { key: 'alpha/session-1', label: 'Rebuild the rig' },
    { key: 'alpha/session-9', label: 'alpha/session-9' },
  ]);
  expect(sessionOptions([held], 'alpha/session-1')).toEqual([
    { key: 'alpha/session-1', label: 'Rebuild the rig' },
  ]);
  expect(sessionOptions([held], '')).toEqual([
    { key: 'alpha/session-1', label: 'Rebuild the rig' },
  ]);
});

/** The order the options keep, which is the order the API answers contexts in. */
test('the options are reordered, so the most recently seen session is not the first offered', () => {
  const rows = [
    contextRow({ key: 'alpha/session-2', name: 'Sort the bins' }),
    contextRow({ key: 'alpha/session-1', name: 'Rebuild the rig' }),
  ];

  expect(sessionOptions(rows, '').map((option) => option.key)).toEqual([
    'alpha/session-2',
    'alpha/session-1',
  ]);
});
