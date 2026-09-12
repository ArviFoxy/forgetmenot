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

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <h1>Contexts</h1>
        <p class="actions">
          <button type="button" class="outline" @click=${() => void this.rows.load(() => api.contexts())}>
            Reload
          </button>
        </p>
      </header>
      ${gate(
        this.rows.state,
        (rows) => html`<figure>
          <table>
            <thead>
              <tr>
                <th scope="col">Key</th>
                <th scope="col">Active scopes</th>
                <th scope="col">Delivered</th>
                <th scope="col">Last seen</th>
              </tr>
            </thead>
            <tbody>
              ${rows.map(
                (row) => html`<tr>
                  <td><code>${row.key}</code></td>
                  <td>
                    ${row.active_scopes.map(
                      (scope) => html`<a class="chip" href=${paths.scope(scope)}>${scope}</a>`,
                    )}
                  </td>
                  <td>${row.delivered_count}</td>
                  <td>${row.last_seen}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </figure>`,
      )}
    `;
  }
}

customElements.define('fmn-contexts-view', FmnContextsView);
