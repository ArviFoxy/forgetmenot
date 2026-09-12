import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { buildTree, filterTree, keysToReveal, type TreeNode } from '../model/tree';
import { currentPath, navigate, onLocationChange, onStoreChange } from '../navigation';
import { paths, resolve } from '../routes';
import './fmn-kind-icon';

const openKeysStorage = 'fmn-tree-open';

const categoryIcons: Record<string, string> = {
  'category:projects': 'folder',
  'category:domains': 'network',
  'category:directories': 'folder-open',
  'category:machines': 'monitor',
  'category:sessions': 'terminal',
  'category:other': 'circle-help',
};

function nodeIcon(node: TreeNode): string {
  if (node.kind === 'category') return categoryIcons[node.key] ?? 'folder';
  if (node.scopeId === 'global') return 'globe';
  if (node.key.startsWith('scope:machine:')) return 'monitor';
  if (node.key.startsWith('scope:session:')) return 'terminal';
  return 'folder';
}

function readOpenKeys(): Set<string> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(openKeysStorage) ?? '[]');
    return new Set(Array.isArray(stored) ? stored.filter((key): key is string => typeof key === 'string') : []);
  } catch {
    return new Set();
  }
}

/** The hierarchy: categories, the scopes in them, and the memories in each scope. */
export class FmnSidebar extends PageElement {
  static override properties: PropertyDeclarations = {
    search: { state: true },
    withArchived: { state: true },
  };

  private search = '';
  private withArchived = false;

  private readonly tree = new Resource<TreeNode[]>(() => this.requestUpdate());
  private openKeys = readOpenKeys();
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
      return buildTree(scopes, memories, contexts.flatMap((context) => context.active_scopes));
    });
  }

  private selection(): { memoryId: string; scopeId: string } {
    const view = resolve(currentPath());
    return { memoryId: view.properties.memoryId ?? '', scopeId: view.properties.scopeId ?? '' };
  }

  private rememberOpen(key: string, open: boolean): void {
    if (open) this.openKeys.add(key);
    else this.openKeys.delete(key);
    window.localStorage.setItem(openKeysStorage, JSON.stringify([...this.openKeys]));
  }

  private renderNode(node: TreeNode, revealed: Set<string>, searching: boolean, selected: { memoryId: string; scopeId: string }): TemplateResult {
    const isSelected =
      (node.memory !== undefined && node.memory.id === selected.memoryId) ||
      (node.memory === undefined && node.scopeId !== undefined && node.scopeId === selected.scopeId);
    const expanded = searching || revealed.has(node.key) || this.openKeys.has(node.key);
    const target =
      node.memory !== undefined
        ? paths.memory(node.memory.id)
        : node.scopeId !== undefined
          ? paths.scope(node.scopeId)
          : '';

    return html`<sl-tree-item
      ?expanded=${expanded}
      ?selected=${isSelected}
      data-key=${node.key}
      data-target=${target}
      @sl-after-expand=${(event: Event) => {
        if (event.target !== event.currentTarget) return;
        this.rememberOpen(node.key, true);
      }}
      @sl-after-collapse=${(event: Event) => {
        if (event.target !== event.currentTarget) return;
        this.rememberOpen(node.key, false);
      }}
    >
      <span class="tree-row ${node.kind}">
        ${node.memory === undefined
          ? html`<sl-icon name=${nodeIcon(node)}></sl-icon>`
          : html`<fmn-kind-icon kind=${node.memory.kind}></fmn-kind-icon>`}
        <span class="tree-label" title=${node.memory?.description ?? node.label}>${node.label}</span>
        ${node.memory?.archived === true
          ? html`<sl-icon name="archive" label="archived"></sl-icon>`
          : nothing}
        ${node.kind === 'memory' ? nothing : html`<span class="tree-count">${node.count}</span>`}
      </span>
      ${node.children.map((child) => this.renderNode(child, revealed, searching, selected))}
    </sl-tree-item>`;
  }

  private renderTree(nodes: TreeNode[]): TemplateResult {
    const selected = this.selection();
    const shown = filterTree(nodes, this.search);
    const searching = this.search.trim() !== '';
    if (shown.length === 0) return html`<p class="tree-empty">No match</p>`;
    const revealed = new Set(keysToReveal(shown, selected.memoryId, selected.scopeId));
    return html`<sl-tree
      class="tree"
      selection="single"
      @sl-selection-change=${(event: CustomEvent<{ selection: HTMLElement[] }>) => {
        const target = event.detail.selection[0]?.dataset.target ?? '';
        if (target !== '' && target !== currentPath()) navigate(target);
      }}
    >
      ${shown.map((node) => this.renderNode(node, revealed, searching, selected))}
    </sl-tree>`;
  }

  override render(): TemplateResult {
    return html`
      <div class="sidebar">
        <div class="sidebar-header">
          <sl-input
            size="small"
            type="search"
            clearable
            placeholder="Search"
            label="Search scopes and memories"
            .value=${this.search}
            @sl-input=${(event: Event) => {
              this.search = (event.target as HTMLInputElement).value;
            }}
          >
            <sl-icon slot="prefix" name="search"></sl-icon>
          </sl-input>
          <div class="sidebar-title">
            <span>Scopes</span>
            <sl-checkbox
              size="small"
              ?checked=${this.withArchived}
              @sl-change=${(event: Event) => {
                this.withArchived = (event.target as HTMLInputElement).checked;
                void this.load();
              }}
              >Archived</sl-checkbox
            >
          </div>
        </div>
        <div class="sidebar-tree">
          ${gate(this.tree.state, (nodes) => this.renderTree(nodes))}
        </div>
      </div>
    `;
  }
}

customElements.define('fmn-sidebar', FmnSidebar);
