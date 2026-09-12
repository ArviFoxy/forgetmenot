import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { MemorySummary, ScopeType } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { buildHierarchy, filterHierarchy, type ScopeNode } from '../model/hierarchy';
import { currentPath, onLocationChange, onStoreChange } from '../navigation';
import { paths, resolve } from '../routes';
import './fmn-kind-icon';

const scopeIcons: Record<ScopeType | 'unknown', string> = {
  global: 'bi-globe',
  machine: 'bi-pc-display',
  session: 'bi-terminal',
  project: 'bi-folder',
  domain: 'bi-diagram-3',
  directory: 'bi-folder2-open',
  unknown: 'bi-question-circle',
};

/** The hierarchy: every scope, the memories under it, and a filter over both. */
export class FmnSidebar extends PageElement {
  static override properties: PropertyDeclarations = {
    open: { type: Boolean, reflect: true },
    search: { state: true },
    withArchived: { state: true },
  };

  open = false;

  private search = '';
  private withArchived = false;

  private readonly tree = new Resource<ScopeNode[]>(() => this.requestUpdate());
  private readonly toggled = new Map<string, boolean>();
  private stopListening: (() => void)[] = [];

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
    this.stopListening = [
      onStoreChange(() => void this.load()),
      onLocationChange(() => this.requestUpdate()),
    ];
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    for (const stop of this.stopListening) stop();
    this.stopListening = [];
  }

  private load(): Promise<void> {
    return this.tree.load(async () => {
      const [scopes, memories, contexts] = await Promise.all([
        api.scopeIndex(),
        api.memoryIndex(this.withArchived ? { archived: true } : undefined),
        api.contexts(),
      ]);
      const active = contexts.flatMap((context) => context.active_scopes);
      return buildHierarchy(scopes, memories, active);
    });
  }

  private selection(): { memoryId: string; scopeId: string } {
    const view = resolve(currentPath());
    return {
      memoryId: view.properties.memoryId ?? '',
      scopeId: view.properties.scopeId ?? '',
    };
  }

  private isOpen(node: ScopeNode, searching: boolean, selected: { memoryId: string; scopeId: string }): boolean {
    const explicit = this.toggled.get(node.id);
    if (explicit !== undefined) return explicit;
    if (searching) return true;
    if (node.id === selected.scopeId) return true;
    return node.memories.some((memory) => memory.id === selected.memoryId);
  }

  private renderMemory(memory: MemorySummary, selectedId: string): TemplateResult {
    const current = memory.id === selectedId;
    return html`<li>
      <a
        class="memory-link ${current ? 'selected' : ''}"
        href=${paths.memory(memory.id)}
        aria-current=${current ? 'page' : 'false'}
        title=${memory.description}
      >
        <fmn-kind-icon kind=${memory.kind}></fmn-kind-icon>
        <span class="memory-name">${memory.name}</span>
        ${memory.archived
          ? html`<i class="bi bi-archive archived-mark" role="img" aria-label="archived" title="archived"></i>`
          : nothing}
      </a>
    </li>`;
  }

  private renderTree(nodes: ScopeNode[]): TemplateResult {
    const selected = this.selection();
    const shown = filterHierarchy(nodes, this.search);
    const searching = this.search.trim() !== '';
    if (shown.length === 0) return html`<p class="empty">No match</p>`;
    return html`<ul class="scope-list">
      ${shown.map(
        (node) => html`<li>
          <details
            ?open=${this.isOpen(node, searching, selected)}
            @toggle=${(event: Event) =>
              this.toggled.set(node.id, (event.target as HTMLDetailsElement).open)}
          >
            <summary class=${node.id === selected.scopeId ? 'selected' : ''}>
              <i class="bi ${scopeIcons[node.type]} scope-icon" role="img" aria-label=${node.type}></i>
              <a class="scope-name" href=${paths.scope(node.id)}>${node.id}</a>
              <span class="scope-count">${node.memories.length}</span>
            </summary>
            <ul class="memory-list">
              ${node.memories.map((memory) => this.renderMemory(memory, selected.memoryId))}
              ${node.memories.length === 0 ? html`<li class="empty">No memories</li>` : nothing}
            </ul>
          </details>
        </li>`,
      )}
    </ul>`;
  }

  override render(): TemplateResult {
    return html`
      <aside class="sidebar" ?data-open=${this.open}>
        <div class="sidebar-tools">
          <input
            type="search"
            id="sidebar-search"
            placeholder="Search"
            aria-label="Search scopes and memories"
            .value=${this.search}
            @input=${(event: Event) => {
              this.search = (event.target as HTMLInputElement).value;
            }}
          />
          <label class="archived-toggle">
            <input
              type="checkbox"
              ?checked=${this.withArchived}
              @change=${(event: Event) => {
                this.withArchived = (event.target as HTMLInputElement).checked;
                void this.load();
              }}
            />
            Archived
          </label>
        </div>
        <nav class="tree" aria-label="Scopes and memories">
          ${gate(this.tree.state, (nodes) => this.renderTree(nodes))}
        </nav>
      </aside>
    `;
  }
}

customElements.define('fmn-sidebar', FmnSidebar);
