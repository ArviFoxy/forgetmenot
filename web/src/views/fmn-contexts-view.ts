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
   * Each subagent directly under the session it runs in, and the sessions by
   * key, so the same set of contexts is always listed in the same order.
   */
  private static grouped(rows: ContextRow[]): ContextRow[] {
    const groups = new Map<string, ContextRow[]>();
    for (const row of rows) {
      const group = row.parent ?? row.key;
      groups.set(group, [...(groups.get(group) ?? []), row]);
    }
    return [...groups.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .flatMap(([, members]) =>
        [...members].sort((left, right) => {
          // The session itself leads its group, whether or not its key sorts first.
          const leftIsSession = (left.parent ?? null) === null;
          const rightIsSession = (right.parent ?? null) === null;
          if (leftIsSession !== rightIsSession) return leftIsSession ? -1 : 1;
          return left.key.localeCompare(right.key);
        }),
      );
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
                    ${row.name}
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
