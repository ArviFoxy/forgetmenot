import { html, type PropertyDeclarations, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type {
  ContextRow,
  DenyDayRow,
  LatencyRow,
  MemoryStatsRow,
  ScopeStatsRow,
  Series,
  StatsQuery,
  Summary,
  TriggerStatsRow,
} from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import {
  cellText,
  memoryColumns,
  memorySort,
  scopeColumns,
  scopeSort,
  sessionColumns,
  sessionOptions,
  sessionRows,
  sessionSort,
  statsAddress,
  statsFilterFromSearch,
  statsQueryOf,
  statsRanges,
  statsSearch,
  summaryHeld,
  summaryTokens,
  tokenPoints,
  triggerColumns,
  type SessionRow,
  type StatsFilter,
  type StatsRange,
} from '../model/stats';
import { formatTokens } from '../model/units';
import { navigate, onLocationChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-data-table';
import '../components/fmn-session-series';
import '../components/fmn-token-series';
import type { RowClick } from '../components/fmn-data-table';

const openStorage = 'fmn-stats-open';

/** The sections of the page, in the order they are drawn. */
const sections = [
  { id: 'summary', label: 'Summary' },
  { id: 'series', label: 'Tokens delivered over time' },
  { id: 'scopes', label: 'Per scope' },
  { id: 'memories', label: 'Per memory' },
  { id: 'sessions', label: 'Sessions' },
  { id: 'health', label: 'Hook health' },
] as const;

type SectionId = (typeof sections)[number]['id'];

/** Every section is open on a first visit but the one that is reference material. */
const openByDefault: Record<SectionId, boolean> = {
  summary: true,
  series: true,
  scopes: true,
  memories: true,
  sessions: true,
  health: false,
};

function readOpen(): Record<SectionId, boolean> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(openStorage) ?? 'null');
    if (stored === null || typeof stored !== 'object') return { ...openByDefault };
    const kept = { ...openByDefault };
    for (const section of sections) {
      const value = (stored as Record<string, unknown>)[section.id];
      if (typeof value === 'boolean') kept[section.id] = value;
    }
    return kept;
  } catch {
    return { ...openByDefault };
  }
}

/** The five figures of the summary row, in the order they are read. */
interface Card {
  label: string;
  value: string;
}

function summaryCards(summary: Summary): Card[] {
  return [
    { label: 'Tokens, last hour', value: formatTokens(summaryTokens(summary, '1h')) },
    { label: 'Tokens, last day', value: formatTokens(summaryTokens(summary, '1d')) },
    { label: 'Tokens, last week', value: formatTokens(summaryTokens(summary, '7d')) },
    { label: 'Held calls today', value: String(summaryHeld(summary, '1d')) },
    { label: 'Live contexts', value: String(summary.live_contexts) },
  ];
}

/**
 * The recorded statistics over one window, for one session and one scope. The
 * range and the two filters are in the address, so a reload shows what is on
 * screen and a link to it shows the same, and every section reads the same window.
 */
export class FmnStatsView extends PageElement {
  static override properties: PropertyDeclarations = {
    filter: { state: true },
    open: { state: true },
  };

  private filter: StatsFilter = statsFilterFromSearch();
  private open: Record<SectionId, boolean> = readOpen();

  private readonly summary = new Resource<Summary>(() => this.requestUpdate());
  private readonly series = new Resource<Series>(() => this.requestUpdate());
  private readonly scopes = new Resource<ScopeStatsRow[]>(() => this.requestUpdate());
  private readonly memories = new Resource<MemoryStatsRow[]>(() => this.requestUpdate());
  private readonly sessions = new Resource<SessionRow[]>(() => this.requestUpdate());
  private readonly triggers = new Resource<TriggerStatsRow[]>(() => this.requestUpdate());
  private readonly denies = new Resource<DenyDayRow[]>(() => this.requestUpdate());
  private readonly latency = new Resource<LatencyRow[]>(() => this.requestUpdate());
  /**
   * The contexts the registry holds, which is what the session select offers,
   * and every scope the log knows, which is what the scope select offers. The
   * sessions come from the contexts and not from the log because only a context
   * carries a name to show, and a session nobody named is not worth offering.
   */
  private readonly sessionNames = new Resource<ContextRow[]>(() => this.requestUpdate());
  private readonly scopeNames = new Resource<ScopeStatsRow[]>(() => this.requestUpdate());

  /** The window the sections were read over, which the per-session chart shares. */
  private window: StatsQuery = {};
  private loaded: string | null = null;
  private loadedNames = false;
  private stopListening: (() => void) | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onLocationChange(() => {
      this.filter = statsFilterFromSearch();
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopListening?.();
    this.stopListening = null;
  }

  override updated(): void {
    if (!this.loadedNames) {
      this.loadedNames = true;
      void this.sessionNames.load(() => api.contexts());
      void this.scopeNames.load(() => api.scopeStats());
      void this.denies.load(() => api.denyStats());
      void this.latency.load(() => api.latencyStats());
    }
    const wanted = statsSearch(this.filter);
    if (this.loaded === wanted) return;
    this.loaded = wanted;
    this.reload();
  }

  /** Every section again, over the window the range names as it stands now. */
  private reload(): void {
    const query = statsQueryOf(this.filter, new Date());
    this.window = query;
    void this.summary.load(() => api.summaryStats());
    void this.series.load(() => api.seriesStats(query));
    void this.scopes.load(() => api.scopeStats(query));
    void this.memories.load(() => api.memoryStats(query));
    void this.triggers.load(() => api.triggerStats(query));
    void this.sessions.load(async () => {
      const [rows, contexts] = await Promise.all([
        api.sessionStats(query),
        api.contexts().catch((): ContextRow[] => []),
      ]);
      return sessionRows(rows, new Map(contexts.map((row) => [row.key, row.last_seen])));
    });
  }

  private show(over: Partial<StatsFilter>): void {
    const next = { ...this.filter, ...over };
    if (statsSearch(next) === statsSearch(this.filter)) return;
    this.filter = next;
    navigate(statsAddress(next));
  }

  private toggleSection(id: SectionId, open: boolean): void {
    if (this.open[id] === open) return;
    this.open = { ...this.open, [id]: open };
    window.localStorage.setItem(openStorage, JSON.stringify(this.open));
  }

  private section(id: SectionId, label: string, body: TemplateResult): TemplateResult {
    return html`<sl-details
      class="stats-section"
      data-section=${id}
      ?open=${this.open[id]}
      @sl-show=${(event: Event) => {
        if (event.target === event.currentTarget) this.toggleSection(id, true);
      }}
      @sl-hide=${(event: Event) => {
        if (event.target === event.currentTarget) this.toggleSection(id, false);
      }}
    >
      <h2 slot="summary">${label}</h2>
      ${body}
    </sl-details>`;
  }

  private renderControls(): TemplateResult {
    const sessions = sessionOptions(this.sessionNames.value ?? [], this.filter.session);
    const scopes = this.scopeNames.value ?? [];
    return html`<div class="actions series-controls">
      <sl-radio-group
        size="small"
        value=${this.filter.range}
        @sl-change=${(event: Event) =>
          this.show({ range: (event.target as HTMLInputElement).value as StatsRange })}
      >
        ${statsRanges.map(
          (range) => html`<sl-radio-button value=${range}>${range}</sl-radio-button>`,
        )}
      </sl-radio-group>
      <sl-select
        class="series-filter"
        size="small"
        placeholder="All sessions"
        value=${this.filter.session}
        @sl-change=${(event: Event) =>
          this.show({ session: (event.target as HTMLInputElement).value })}
      >
        <sl-option value="">All sessions</sl-option>
        ${sessions.map(
          (option) => html`<sl-option value=${option.key}>${option.label}</sl-option>`,
        )}
      </sl-select>
      <sl-select
        class="series-filter"
        size="small"
        placeholder="All scopes"
        value=${this.filter.scope}
        @sl-change=${(event: Event) =>
          this.show({ scope: (event.target as HTMLInputElement).value })}
      >
        <sl-option value="">All scopes</sl-option>
        ${scopes.map((row) => html`<sl-option value=${row.scope_id}>${row.scope_id}</sl-option>`)}
      </sl-select>
    </div>`;
  }

  private renderSeries(series: Series): TemplateResult {
    if (series.points.length === 0) return html`<p class="empty">Nothing in this range</p>`;
    return html`<fmn-token-series
      .points=${tokenPoints(series.points)}
      .from=${this.window.from === undefined ? null : new Date(this.window.from)}
      .to=${this.window.to === undefined ? null : new Date(this.window.to)}
    ></fmn-token-series>`;
  }

  /** The triggers of one scope, as a plain table under the row that opened it. */
  private renderScopeTriggers(scope: string): TemplateResult {
    const rows = (this.triggers.value ?? []).filter((row) => row.scope_id === scope);
    if (rows.length === 0) return html`<p class="empty">Nothing in this range</p>`;
    return html`<table class="data stats sub">
      <thead>
        <tr>
          ${triggerColumns.map(
            (column) =>
              html`<th scope="col" class=${column.numeric === true ? 'number' : ''}>
                ${column.header}
              </th>`,
          )}
        </tr>
      </thead>
      <tbody>
        ${rows.map(
          (row) => html`<tr>
            ${triggerColumns.map(
              (column) =>
                html`<td class=${column.numeric === true ? 'number' : ''}>
                  ${column.mono === true
                    ? html`<code>${cellText(column, row)}</code>`
                    : cellText(column, row)}
                </td>`,
            )}
          </tr>`,
        )}
      </tbody>
    </table>`;
  }

  private renderHealth(): TemplateResult {
    return html`
      <h3>Hook latency</h3>
      ${gate(
        this.latency.state,
        (rows) => html`<div class="table-wrap">
          <table class="data stats latency">
            <thead>
              <tr>
                <th scope="col">Event</th>
                <th scope="col" class="number">Count</th>
                <th scope="col" class="number">p50 (µs)</th>
                <th scope="col" class="number">p90 (µs)</th>
                <th scope="col" class="number">p99 (µs)</th>
                <th scope="col" class="number">Max (µs)</th>
              </tr>
            </thead>
            <tbody>
              ${rows.map(
                (row) => html`<tr>
                  <td>${row.event}</td>
                  <td class="number">${row.count}</td>
                  <td class="number">${row.p50_us}</td>
                  <td class="number">${row.p90_us}</td>
                  <td class="number">${row.p99_us}</td>
                  <td class="number">${row.max_us}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </div>`,
      )}
      <h3>Stopped calls per day</h3>
      ${gate(
        this.denies.state,
        (rows) => html`<div class="table-wrap">
          <table class="data stats">
            <thead>
              <tr>
                <th scope="col">Day (UTC)</th>
                <th scope="col" class="number">Stopped calls</th>
                <th scope="col" class="number">Events</th>
              </tr>
            </thead>
            <tbody>
              ${rows.map(
                (row) => html`<tr>
                  <td class="moment nowrap">${row.day}</td>
                  <td class="number">${row.denies}</td>
                  <td class="number">${row.events}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </div>`,
      )}
    `;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="chart-bar"></sl-icon><span>statistics</span></div>
        <h1>Statistics</h1>
      </header>
      ${this.section(
        'summary',
        'Summary',
        gate(
          this.summary.state,
          (summary) => html`<div class="summary-row">
            ${summaryCards(summary).map(
              (card) => html`<div class="summary-card">
                <span class="summary-value">${card.value}</span>
                <span class="summary-label">${card.label}</span>
              </div>`,
            )}
          </div>`,
        ),
      )}
      ${this.section(
        'series',
        'Tokens delivered over time',
        html`${this.renderControls()}${gate(this.series.state, (series) =>
          this.renderSeries(series),
        )}`,
      )}
      ${this.section(
        'scopes',
        'Per scope',
        gate(
          this.scopes.state,
          (rows) => html`
            <fmn-data-table
              class="stats-scopes"
              .columns=${scopeColumns}
              .sort=${scopeSort}
              .rows=${rows}
              .rowKey=${(row: ScopeStatsRow) => row.scope_id}
              .expand=${(row: ScopeStatsRow) => this.renderScopeTriggers(row.scope_id)}
              filterLabel="Filter scopes"
              @fmn-row-click=${(event: CustomEvent<RowClick<ScopeStatsRow>>) =>
                this.show({ scope: event.detail.row.scope_id })}
            ></fmn-data-table>
            <p class="muted">A memory in several scopes is counted under the one it was printed in.</p>
          `,
        ),
      )}
      ${this.section(
        'memories',
        'Per memory',
        gate(
          this.memories.state,
          (rows) => html`<fmn-data-table
            class="stats-memories"
            .columns=${memoryColumns}
            .sort=${memorySort}
            .rows=${rows}
            .rowKey=${(row: MemoryStatsRow) => row.memory}
            filterLabel="Filter memories"
            @fmn-row-click=${(event: CustomEvent<RowClick<MemoryStatsRow>>) =>
              navigate(paths.memory(event.detail.row.memory))}
          ></fmn-data-table>`,
        ),
      )}
      ${this.section(
        'sessions',
        'Sessions',
        gate(
          this.sessions.state,
          (rows) => html`<fmn-data-table
            class="stats-sessions"
            .columns=${sessionColumns}
            .sort=${sessionSort}
            .rows=${rows}
            .rowKey=${(row: SessionRow) => row.session_key}
            .expand=${(row: SessionRow) =>
              html`<fmn-session-series
                .sessionKey=${row.session_key}
                .window=${this.window}
              ></fmn-session-series>`}
            filterLabel="Filter sessions"
          ></fmn-data-table>`,
        ),
      )}
      ${this.section('health', 'Hook health', this.renderHealth())}
    `;
  }
}

customElements.define('fmn-stats-view', FmnStatsView);
