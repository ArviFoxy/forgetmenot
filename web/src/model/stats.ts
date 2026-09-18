// The parts of the statistics page that are arithmetic on values rather than
// markup: the window a range name stands for, the address the controls are kept
// in, the points the charts are drawn from, and what each table's columns hold.

import type {
  ContextRow,
  MemoryStatsRow,
  ScopeStatsRow,
  SeriesPoint,
  SessionEventPoint,
  SessionStatsRow,
  StatsQuery,
  Summary,
  TriggerStatsRow,
} from '../api/types';
import { formatTokens } from './units';
import { paths } from '../routes';

/** How far back the page looks. `all` reads the whole log. */
export type StatsRange = '24h' | '7d' | '30d' | 'all';

/** The ranges the control offers, in the order it offers them. */
export const statsRanges: StatsRange[] = ['24h', '7d', '30d', 'all'];

const rangeSeconds: Record<Exclude<StatsRange, 'all'>, number> = {
  '24h': 86_400,
  '7d': 604_800,
  '30d': 2_592_000,
};

/** What the page is looking at: one window, one session, one scope. */
export interface StatsFilter {
  range: StatsRange;
  /** A context key, empty for every session. */
  session: string;
  /** A scope id, empty for every scope. */
  scope: string;
}

export const defaultStatsFilter: StatsFilter = { range: '24h', session: '', scope: '' };

/**
 * The window a range covers, ending at `now`. `all` has no window: the request
 * names no bounds and the server reads the whole log.
 */
export function statsWindow(range: StatsRange, now: Date): StatsQuery {
  if (range === 'all') return {};
  const to = now.getTime();
  return {
    from: new Date(to - rangeSeconds[range] * 1000).toISOString(),
    to: new Date(to).toISOString(),
  };
}

/** The window and the filters as one query, which is what every request takes. */
export function statsQueryOf(filter: StatsFilter, now: Date): StatsQuery {
  const query: StatsQuery = statsWindow(filter.range, now);
  if (filter.session !== '') query.session = filter.session;
  if (filter.scope !== '') query.scope = filter.scope;
  return query;
}

function readRange(text: string | null): StatsRange {
  return statsRanges.find((range) => range === text) ?? defaultStatsFilter.range;
}

/** What the address asks for, as `statsSearch` wrote it. */
export function statsFilterFromSearch(search: string = window.location.search): StatsFilter {
  const query = new URLSearchParams(search);
  return {
    range: readRange(query.get('range')),
    session: query.get('session') ?? '',
    scope: query.get('scope') ?? '',
  };
}

/**
 * The query part of the address for a filter. What the page would show anyway is
 * left out, so the plain address is the default view.
 */
export function statsSearch(filter: StatsFilter): string {
  const query = new URLSearchParams();
  if (filter.range !== defaultStatsFilter.range) query.set('range', filter.range);
  if (filter.session !== '') query.set('session', filter.session);
  if (filter.scope !== '') query.set('scope', filter.scope);
  const text = query.toString();
  return text === '' ? '' : `?${text}`;
}

/** The address that shows this filter, which is what a control changes to. */
export function statsAddress(filter: StatsFilter): string {
  return `${paths.stats()}${statsSearch(filter)}`;
}

/** One point of a chart: an instant and the figures drawn at it. */
export interface TokenPoint {
  at: Date;
  tokens: number;
}

/**
 * The series as Plot reads it. The bucket start is an instant, so the x scale is
 * time rather than a band of one category per bucket.
 */
export function tokenPoints(points: SeriesPoint[]): TokenPoint[] {
  return points.map((point) => ({ at: new Date(point.t), tokens: point.tokens }));
}

/** One hook event of a session, as the per-session chart draws it. */
export interface SessionPoint {
  at: Date;
  event: string;
  /** The size Claude Code reported; null when this event carried none. */
  contextTokens: number | null;
  tokens: number;
}

export function sessionPoints(rows: SessionEventPoint[]): SessionPoint[] {
  return rows.map((row) => ({
    at: new Date(row.t),
    event: row.event,
    contextTokens: row.context_tokens,
    tokens: row.tokens,
  }));
}

/**
 * The events that reported a context size. A line through a missing size would
 * join the sizes on either side of it and draw a measurement nobody made.
 */
export function measuredSizes(points: SessionPoint[]): { at: Date; contextTokens: number }[] {
  return points
    .filter((point) => point.contextTokens !== null)
    .map((point) => ({ at: point.at, contextTokens: point.contextTokens ?? 0 }));
}

/** The figure the summary shows for one window, `0` when the answer has no such window. */
export function summaryTokens(summary: Summary, window: string): number {
  return summary.windows.find((row) => row.name === window)?.tokens ?? 0;
}

/** Tool calls held in one window, `0` when the answer has no such window. */
export function summaryHeld(summary: Summary, window: string): number {
  return summary.windows.find((row) => row.name === window)?.held ?? 0;
}

/**
 * How narrow a screen may be and still show a column: 1 is drawn at every width,
 * 2 down to 600 px, 3 down to 900 px.
 */
export type ColumnPriority = 1 | 2 | 3;

/** The least viewport width, in pixels, at which a column of each priority is drawn. */
const priorityWidths: Record<ColumnPriority, number> = { 1: 0, 2: 600, 3: 900 };

/** The widths at which the columns shown change, which is what a table listens for. */
export const columnBreakpoints: number[] = [600, 900];

/** One column of a table: what it is called, what it holds, and where it leads. */
export interface TableColumn<Row> {
  id: string;
  header: string;
  /** What the column sorts and filters by. */
  value: (row: Row) => string | number;
  /** The text the cell shows; the value itself when this is absent. */
  text?: (row: Row) => string;
  /** The address the cell links to, null for a cell that is not a link. */
  link?: (row: Row) => string | null;
  /** Figures, which are set to the right in digits of one width. */
  numeric?: boolean;
  /** Text that is read character by character, such as a pattern. */
  mono?: boolean;
  /** A value the clock decides, which the recorded screenshots paint over. */
  moment?: boolean;
  /** How narrow a screen may be and still show it; every width when absent. */
  priority?: ColumnPriority;
}

/** The columns a screen `width` pixels across shows. */
export function visibleColumns<Row>(
  columns: TableColumn<Row>[],
  width: number,
): TableColumn<Row>[] {
  return columns.filter((column) => width >= priorityWidths[column.priority ?? 1]);
}

/** The columns a screen `width` pixels across leaves out, which an open row lists. */
export function hiddenColumns<Row>(columns: TableColumn<Row>[], width: number): TableColumn<Row>[] {
  return columns.filter((column) => width < priorityWidths[column.priority ?? 1]);
}

/** What a table is sorted by until a header is clicked. */
export interface TableSort {
  /** The id of the column sorted by. */
  id: string;
  /** Largest first. */
  desc: boolean;
}

/** The scope that cost the most is the row a reader of the table is looking for. */
export const scopeSort: TableSort = { id: 'tokens', desc: true };
export const memorySort: TableSort = { id: 'tokens', desc: true };
export const sessionSort: TableSort = { id: 'tokens', desc: true };

/** The text a cell shows, which is its own when it has one and its value otherwise. */
export function cellText<Row>(column: TableColumn<Row>, row: Row): string {
  return column.text?.(row) ?? String(column.value(row));
}

export const scopeColumns: TableColumn<ScopeStatsRow>[] = [
  {
    id: 'scope',
    header: 'Scope',
    value: (row) => row.scope_id,
    link: (row) => paths.scope(row.scope_id),
  },
  { id: 'tokens', header: 'Tokens', value: (row) => row.tokens, text: (row) => formatTokens(row.tokens), numeric: true },
  {
    id: 'deliveries',
    header: 'Deliveries',
    value: (row) => row.deliveries,
    numeric: true,
    priority: 2,
  },
  {
    id: 'activations',
    header: 'Activations',
    value: (row) => row.activations,
    numeric: true,
    priority: 2,
  },
  {
    id: 'forgettings',
    header: 'Forgettings',
    value: (row) => row.forgettings,
    numeric: true,
    priority: 3,
  },
  {
    id: 'live',
    header: 'Live contexts',
    value: (row) => row.live_contexts,
    numeric: true,
    priority: 3,
  },
  {
    id: 'memories',
    header: 'Memories',
    value: (row) => row.memories,
    numeric: true,
    priority: 3,
  },
  {
    id: 'per-delivery',
    header: 'Tokens per delivery',
    value: (row) => row.tokens_per_delivery,
    text: (row) => row.tokens_per_delivery.toFixed(0),
    numeric: true,
    priority: 3,
  },
];

export const memoryColumns: TableColumn<MemoryStatsRow>[] = [
  {
    id: 'memory',
    header: 'Memory',
    value: (row) => row.memory,
    link: (row) => paths.memory(row.memory),
  },
  { id: 'tokens', header: 'Tokens', value: (row) => row.tokens, text: (row) => formatTokens(row.tokens), numeric: true },
  { id: 'new', header: 'New', value: (row) => row.shown_full_new, numeric: true, priority: 3 },
  {
    id: 'changed',
    header: 'Changed',
    value: (row) => row.shown_full_changed,
    numeric: true,
    priority: 3,
  },
  {
    id: 'stale',
    header: 'Stale',
    value: (row) => row.shown_full_stale,
    numeric: true,
    priority: 3,
  },
  { id: 'shrunk', header: 'Shrunk', value: (row) => row.shrunk, numeric: true, priority: 3 },
  {
    id: 'fetched',
    header: 'Fetched',
    value: (row) => row.fetched_full,
    numeric: true,
    priority: 2,
  },
  {
    id: 'last-shown',
    header: 'Last shown',
    value: (row) => row.last_shown ?? '',
    moment: true,
    priority: 3,
  },
  { id: 'most-under', header: 'Printed under', value: (row) => row.most_under ?? '', priority: 3 },
];

/** What a session row holds, which is the log's row and when the context was last seen. */
export interface SessionRow extends SessionStatsRow {
  /** When the registry last saw this context; empty for one it no longer holds. */
  last_seen: string;
}

export const sessionColumns: TableColumn<SessionRow>[] = [
  {
    id: 'session',
    header: 'Session',
    value: (row) => row.session_key,
    link: (row) => paths.contextPrompt(row.session_key),
  },
  { id: 'tokens', header: 'Tokens', value: (row) => row.tokens, text: (row) => formatTokens(row.tokens), numeric: true },
  {
    id: 'context',
    header: 'Last context size',
    value: (row) => row.last_context_tokens ?? 0,
    text: (row) => (row.last_context_tokens === null ? '' : formatTokens(row.last_context_tokens)),
    numeric: true,
    priority: 2,
  },
  {
    id: 'last-seen',
    header: 'Last seen',
    value: (row) => row.last_seen,
    moment: true,
    priority: 3,
  },
];

export const triggerColumns: TableColumn<TriggerStatsRow>[] = [
  { id: 'field', header: 'Field', value: (row) => row.field },
  { id: 'pattern', header: 'Pattern', value: (row) => row.pattern, mono: true },
  { id: 'fires', header: 'Fires', value: (row) => row.fires, numeric: true },
  { id: 'new-activations', header: 'New activations', value: (row) => row.new_activations, numeric: true },
  {
    id: 'deny-share',
    header: 'Share that held a call',
    value: (row) => row.deny_share,
    text: (row) => row.deny_share.toFixed(2),
    numeric: true,
  },
];

/** One session the page's session filter offers. */
export interface SessionOption {
  /** The context key the filter takes, which is what the address carries. */
  key: string;
  /** What the option reads as. */
  label: string;
}

/**
 * The sessions the page offers to filter by: the main contexts that have a name
 * of their own, labelled by that name, in the order `/api/contexts` answers
 * them, which is the most recently seen first.
 *
 * A subagent is left out because it is not a session, and a context whose only
 * name is its key is left out because an option reading as a key is what naming
 * the sessions replaced. `selected` is the key the address names: it stays on
 * offer, labelled by itself, whatever the contexts hold, so that a link to a
 * session the registry no longer knows still shows the filter it asked for.
 */
export function sessionOptions(contexts: ContextRow[], selected: string): SessionOption[] {
  const options = contexts
    .filter((row) => row.parent === null && row.name !== '' && row.name !== row.key)
    .map((row) => ({ key: row.key, label: row.name }));
  if (selected !== '' && !options.some((option) => option.key === selected)) {
    options.push({ key: selected, label: selected });
  }
  return options;
}

/** The session rows with the time the registry last saw each context. */
export function sessionRows(rows: SessionStatsRow[], lastSeen: Map<string, string>): SessionRow[] {
  return rows.map((row) => ({ ...row, last_seen: lastSeen.get(row.session_key) ?? '' }));
}
