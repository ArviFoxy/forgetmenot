import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type { ContextRow } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { paths } from '../routes';

/** The live contexts: what each one has active and when it was last seen. */
export class FmnContextsView extends PageElement {
  private readonly rows = new Resource<ContextRow[]>(() => this.requestUpdate());
  private started = false;

  override updated(): void {
    if (this.started) return;
    this.started = true;
    void this.rows.load(() => api.contexts());
  }

  /** By key, so the same set of contexts is always listed in the same order. */
  private static byKey(rows: ContextRow[]): ContextRow[] {
    return [...rows].sort((left, right) => left.key.localeCompare(right.key));
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="activity"></sl-icon><span>contexts</span></div>
        <h1>Contexts</h1>
        <div class="actions">
          <sl-button size="small" @click=${() => void this.rows.load(() => api.contexts())}>
            <sl-icon slot="prefix" name="refresh-cw"></sl-icon>Reload
          </sl-button>
        </div>
      </header>
      ${gate(
        this.rows.state,
        (loaded) => html`<div class="table-wrap">
          <table class="data">
            <thead>
              <tr>
                <th scope="col">Key</th>
                <th scope="col">Active scopes</th>
                <th scope="col" class="number">Delivered</th>
                <th scope="col">Last seen</th>
              </tr>
            </thead>
            <tbody>
              ${FmnContextsView.byKey(loaded).map(
                (row) => html`<tr>
                  <td class="nowrap"><code>${row.key}</code></td>
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
                  <td class="number">${row.delivered_count}</td>
                  <td class="nowrap">${row.last_seen}</td>
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
