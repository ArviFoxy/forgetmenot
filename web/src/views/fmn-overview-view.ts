import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type { MemorySummary, ScopeDoc } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-kind-icon';

/** Every memory and every scope with a file, as two tables. */
export class FmnOverviewView extends PageElement {
  private readonly memories = new Resource<MemorySummary[]>(() => this.requestUpdate());
  private readonly scopes = new Resource<ScopeDoc[]>(() => this.requestUpdate());
  private started = false;
  private stopListening: (() => void) | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onStoreChange(() => this.load());
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopListening?.();
    this.stopListening = null;
  }

  private load(): void {
    void this.memories.load(() => api.memoryIndex({ archived: true }));
    void this.scopes.load(() => api.scopeIndex());
  }

  override updated(): void {
    if (this.started) return;
    this.started = true;
    this.load();
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="file-text"></sl-icon><span>overview</span></div>
        <h1>Memories and scopes</h1>
      </header>

      <h2>Memories</h2>
      ${gate(
        this.memories.state,
        (rows) => html`<div class="table-wrap">
          <table class="data">
            <thead>
              <tr>
                <th scope="col">Memory</th>
                <th scope="col">Description</th>
                <th scope="col">Kind</th>
                <th scope="col">Scopes</th>
                <th scope="col">Modified</th>
              </tr>
            </thead>
            <tbody>
              ${rows.map(
                (row) => html`<tr>
                  <td class="nowrap"><a href=${paths.memory(row.id)}>${row.name}</a></td>
                  <td>${row.description}</td>
                  <td class="nowrap">
                    <span class="row-icon"
                      ><fmn-kind-icon kind=${row.kind}></fmn-kind-icon>${row.kind}</span
                    >
                  </td>
                  <td>
                    <span class="chips"
                      >${row.scopes.map(
                        (scope) =>
                          html`<a href=${paths.scope(scope)}
                            ><sl-badge variant="neutral" pill>${scope}</sl-badge></a
                          >`,
                      )}</span
                    >
                  </td>
                  <td class="nowrap">${row.modified ?? '—'}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </div>`,
      )}

      <h2>Scopes</h2>
      ${gate(
        this.scopes.state,
        (rows) => html`<div class="table-wrap">
          <table class="data">
            <thead>
              <tr>
                <th scope="col">Scope</th>
                <th scope="col">Type</th>
                <th scope="col">Implies</th>
                <th scope="col" class="number">Triggers</th>
              </tr>
            </thead>
            <tbody>
              ${rows.map(
                (row) => html`<tr>
                  <td class="nowrap"><a href=${paths.scope(row.id)}>${row.id}</a></td>
                  <td class="nowrap">${row.type}</td>
                  <td>
                    <span class="chips"
                      >${row.implies.map(
                        (other) =>
                          html`<a href=${paths.scope(other)}
                            ><sl-badge variant="neutral" pill>${other}</sl-badge></a
                          >`,
                      )}</span
                    >
                  </td>
                  <td class="number">${row.triggers.length}</td>
                </tr>`,
              )}
            </tbody>
          </table>
        </div>`,
      )}
    `;
  }
}

customElements.define('fmn-overview-view', FmnOverviewView);
