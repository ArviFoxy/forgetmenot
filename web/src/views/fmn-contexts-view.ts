import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { contextColumns, contextTree, type ContextNode } from '../model/contexts';
import '../components/fmn-data-table';

/**
 * The live contexts: what each one has active and when it was last seen, with
 * every context under the one that spawned it, closed until it is opened.
 */
export class FmnContextsView extends PageElement {
  private readonly rows = new Resource<ContextNode[]>(() => this.requestUpdate());
  private started = false;

  /** The tree, built once per read, so the table is given the same rows each render. */
  private load(): Promise<void> {
    return this.rows.load(async () => contextTree(await api.contexts()));
  }

  override updated(): void {
    if (this.started) return;
    this.started = true;
    void this.load();
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="activity"></sl-icon><span>contexts</span></div>
        <h1>Contexts</h1>
        <div class="actions">
          <sl-button size="small" @click=${() => void this.load()}>
            <sl-icon slot="prefix" name="refresh"></sl-icon>Reload
          </sl-button>
        </div>
      </header>
      ${gate(
        this.rows.state,
        (loaded) => html`<fmn-data-table
          class="contexts-table"
          .columns=${contextColumns}
          .rows=${loaded}
          .rowKey=${(row: ContextNode) => row.key}
          .subRows=${(row: ContextNode) => row.children}
          empty="No live contexts"
          filterLabel="Filter contexts"
        ></fmn-data-table>`,
      )}
    `;
  }
}

customElements.define('fmn-contexts-view', FmnContextsView);
