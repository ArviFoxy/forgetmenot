import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type {
  DenyDayRow,
  LatencyRow,
  MemoryStatsRow,
  ScopeStatsRow,
  SessionBytesRow,
  TriggerStatsRow,
} from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { formatBytes } from '../model/units';
import { paths } from '../routes';

/**
 * The recorded statistics. Index-line deliveries and full deliveries are separate
 * counts in their own columns, because they cost different amounts of context and
 * adding them together would hide which of the two happened.
 */
export class FmnStatsView extends PageElement {
  private readonly memories = new Resource<MemoryStatsRow[]>(() => this.requestUpdate());
  private readonly triggers = new Resource<TriggerStatsRow[]>(() => this.requestUpdate());
  private readonly scopes = new Resource<ScopeStatsRow[]>(() => this.requestUpdate());
  private readonly denies = new Resource<DenyDayRow[]>(() => this.requestUpdate());
  private readonly latency = new Resource<LatencyRow[]>(() => this.requestUpdate());
  private readonly sessions = new Resource<SessionBytesRow[]>(() => this.requestUpdate());
  private started = false;

  override updated(): void {
    if (this.started) return;
    this.started = true;
    void this.memories.load(() => api.memoryStats());
    void this.triggers.load(() => api.triggerStats());
    void this.scopes.load(() => api.scopeStats());
    void this.denies.load(() => api.denyStats());
    void this.latency.load(() => api.latencyStats());
    void this.sessions.load(() => api.sessionStats());
  }

  /** Rows in a fixed order: the server is free to answer in any. */
  private static sorted<Row>(rows: Row[], key: (row: Row) => string): Row[] {
    return [...rows].sort((left, right) => key(left).localeCompare(key(right)));
  }

  private renderMemories(rows: MemoryStatsRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats">
        <thead>
          <tr>
            <th scope="col">Memory</th>
            <th scope="col">Shown as index line</th>
            <th scope="col">Shown in full, new</th>
            <th scope="col">Shown in full, changed</th>
            <th scope="col">Shown in full, stale</th>
            <th scope="col">Fetched in full by the model</th>
            <th scope="col">Retracted</th>
            <th scope="col">Last shown</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => row.memory).map(
            (row) => html`<tr>
              <td><a href=${paths.memory(row.memory)}>${row.memory}</a></td>
              <td class="number">${row.shown_index}</td>
              <td class="number">${row.shown_full_new}</td>
              <td class="number">${row.shown_full_changed}</td>
              <td class="number">${row.shown_full_stale}</td>
              <td class="number">${row.fetched_full}</td>
              <td class="number">${row.retracted}</td>
              <td class="nowrap moment">${row.last_shown ?? ''}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }

  private renderTriggers(rows: TriggerStatsRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats">
        <thead>
          <tr>
            <th scope="col">Scope</th>
            <th scope="col">Field</th>
            <th scope="col">Pattern</th>
            <th scope="col">Fires</th>
            <th scope="col">New activations</th>
            <th scope="col">Share of fires that stopped a call</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => `${row.scope_id} ${row.field} ${row.pattern}`).map(
            (row) => html`<tr>
              <td><a href=${paths.scope(row.scope_id)}>${row.scope_id}</a></td>
              <td>${row.field}</td>
              <td><code>${row.pattern}</code></td>
              <td class="number">${row.fires}</td>
              <td class="number">${row.new_activations}</td>
              <td class="number">${row.deny_share.toFixed(2)}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }

  private renderScopes(rows: ScopeStatsRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats">
        <thead>
          <tr>
            <th scope="col">Scope</th>
            <th scope="col">Activations</th>
            <th scope="col">Forgettings</th>
            <th scope="col">Live contexts</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => row.scope_id).map(
            (row) => html`<tr>
              <td><a href=${paths.scope(row.scope_id)}>${row.scope_id}</a></td>
              <td class="number">${row.activations}</td>
              <td class="number">${row.forgettings}</td>
              <td class="number">${row.live_contexts}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }

  private renderDenies(rows: DenyDayRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats">
        <thead>
          <tr>
            <th scope="col">Day (UTC)</th>
            <th scope="col">Stopped calls</th>
            <th scope="col">Events</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => row.day).map(
            (row) => html`<tr>
              <td class="moment">${row.day}</td>
              <td class="number">${row.denies}</td>
              <td class="number">${row.events}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }

  private renderLatency(rows: LatencyRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats latency">
        <thead>
          <tr>
            <th scope="col">Event</th>
            <th scope="col">Count</th>
            <th scope="col">p50 (µs)</th>
            <th scope="col">p90 (µs)</th>
            <th scope="col">p99 (µs)</th>
            <th scope="col">Max (µs)</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => row.event).map(
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
    </div>`;
  }

  /** The byte counts in the unit that fits, with the exact count in a `title`. */
  private renderSessions(rows: SessionBytesRow[]): TemplateResult {
    return html`<div class="table-wrap">
      <table class="data stats">
        <thead>
          <tr>
            <th scope="col">Session</th>
            <th scope="col">Bytes shown in full</th>
            <th scope="col">Bytes shown as index lines</th>
          </tr>
        </thead>
        <tbody>
          ${FmnStatsView.sorted(rows, (row) => row.session_key).map(
            (row) => html`<tr>
              <td><code>${row.session_key}</code></td>
              <td class="number" title=${row.bytes_full}>${formatBytes(row.bytes_full)}</td>
              <td class="number" title=${row.bytes_index}>${formatBytes(row.bytes_index)}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="chart-bar"></sl-icon><span>statistics</span></div>
        <h1>Statistics</h1>
      </header>
      <h2>Per memory</h2>
      ${gate(this.memories.state, (rows) => this.renderMemories(rows))}
      <h2>Per trigger</h2>
      ${gate(this.triggers.state, (rows) => this.renderTriggers(rows))}
      <h2>Per scope</h2>
      ${gate(this.scopes.state, (rows) => this.renderScopes(rows))}
      <h2>Stopped calls per day</h2>
      ${gate(this.denies.state, (rows) => this.renderDenies(rows))}
      <h2>Hook latency</h2>
      ${gate(this.latency.state, (rows) => this.renderLatency(rows))}
      <h2>Delivered bytes per session</h2>
      ${gate(this.sessions.state, (rows) => this.renderSessions(rows))}
    `;
  }
}

customElements.define('fmn-stats-view', FmnStatsView);
