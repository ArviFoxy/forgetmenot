import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type { ContextRow } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { paths } from '../routes';

/** The machine part of a context key, which is everything before the first `/`. */
function machineOf(key: string): string {
  const slash = key.indexOf('/');
  return slash === -1 ? key : key.slice(0, slash);
}

/**
 * The rest of a context key: `<session-id>`, or `<session-id>/<agent-id>` for a
 * subagent.
 */
function idOf(key: string): string {
  const slash = key.indexOf('/');
  return slash === -1 ? '' : key.slice(slash + 1);
}

/**
 * What the Name cell reads: the name the server derived, or the context's own id
 * when nothing but the key is known about it, so that every row has text to open
 * the context's prompt by.
 */
function nameOf(row: ContextRow): string {
  return row.name === '' ? idOf(row.key) : row.name;
}

/** The live contexts: what each one has active and when it was last seen. */
export class FmnContextsView extends PageElement {
  private readonly rows = new Resource<ContextRow[]>(() => this.requestUpdate());
  private started = false;

  override updated(): void {
    if (this.started) return;
    this.started = true;
    void this.rows.load(() => api.contexts());
  }

  /**
   * Each subagent directly under the session it runs in, in the order the rows
   * arrived in: a session's group stands where the first of its rows stands, so
   * a group sent in most-recently-seen order is placed by the member seen last,
   * and the subagents of one session keep the order they came in.
   */
  private static grouped(rows: ContextRow[]): ContextRow[] {
    const groups = new Map<string, ContextRow[]>();
    for (const row of rows) {
      const group = row.parent ?? row.key;
      groups.set(group, [...(groups.get(group) ?? []), row]);
    }
    // The session itself leads its group, whether or not it was seen last.
    return [...groups.values()].flatMap((members) => [
      ...members.filter((row) => (row.parent ?? null) === null),
      ...members.filter((row) => (row.parent ?? null) !== null),
    ]);
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="activity"></sl-icon><span>contexts</span></div>
        <h1>Contexts</h1>
        <div class="actions">
          <sl-button size="small" @click=${() => void this.rows.load(() => api.contexts())}>
            <sl-icon slot="prefix" name="refresh"></sl-icon>Reload
          </sl-button>
        </div>
      </header>
      ${gate(
        this.rows.state,
        (loaded) => html`<div class="table-wrap">
          <table class="data">
            <thead>
              <tr>
                <th scope="col" class="context-name">Name</th>
                <th scope="col">Scopes</th>
                <th scope="col">Id</th>
                <th scope="col">Machine</th>
                <th scope="col" class="number">Delivered</th>
                <th scope="col">Last seen</th>
              </tr>
            </thead>
            <tbody>
              ${FmnContextsView.grouped(loaded).map(
                (row) => html`<tr>
                  <td
                    class=${(row.parent ?? null) === null ? 'context-name' : 'context-name nested'}
                  >
                    <a href=${paths.contextPrompt(row.key)}>${nameOf(row)}</a>
                  </td>
                  <td>
                    <span class="chips"
                      >${[...row.active_scopes].sort().map(
                        (scope) =>
                          html`<a href=${paths.scope(scope)}
                            ><sl-badge variant="neutral" pill>${scope}</sl-badge></a
                          >`,
                      )}</span
                    >
                  </td>
                  <td class="nowrap"><code class="value-mono">${idOf(row.key)}</code></td>
                  <td class="nowrap">${machineOf(row.key)}</td>
                  <td class="number">${row.delivered_count}</td>
                  <td class="nowrap moment">${row.last_seen}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </div>`,
      )}
    `;
  }
}

customElements.define('fmn-contexts-view', FmnContextsView);
