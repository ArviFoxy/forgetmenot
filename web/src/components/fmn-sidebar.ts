import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { buildTree, filterTree, keysToReveal, scopeKind, type TreeNode } from '../model/tree';
import { currentPath, navigate, onLocationChange, onStoreChange } from '../navigation';
import { requestDelete } from '../intent';
import { paths, resolve } from '../routes';
import './fmn-kind-icon';

const openKeysStorage = 'fmn-tree-open';

/** The icon says what kind of scope a row is, which the flat list does not group by. */
function nodeIcon(node: TreeNode): string {
  if (node.kind === 'category') return node.key === 'category:sessions' ? 'terminal' : 'device-desktop';
  if (node.scopeId === undefined) return 'folder';
  const kind = scopeKind(node.scopeId);
  if (kind === 'global') return 'world';
  if (kind === 'machine') return 'device-desktop';
  if (kind === 'session') return 'terminal';
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

/** One line of the menu that a right click opens. */
interface MenuItem {
  value: string;
  label: string;
  icon: string;
}

interface OpenMenu {
  x: number;
  y: number;
  items: MenuItem[];
}

const newItems: MenuItem[] = [
  { value: 'new-scope', label: 'New scope', icon: 'folder' },
  { value: 'new-memory', label: 'New memory', icon: 'file-text' },
];

/** The hierarchy: the scopes, the memories in each, and the session scopes together. */
export class FmnSidebar extends PageElement {
  static override properties: PropertyDeclarations = {
    search: { state: true },
    menu: { state: true },
  };

  private search = '';
  private menu: OpenMenu | null = null;

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
        api.memoryIndex(),
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

  /** What a right click offers on a node, and on the empty space below the tree. */
  private itemsFor(node: TreeNode | null): MenuItem[] {
    if (node === null || node.kind === 'category') return newItems;
    const memory = node.memory;
    if (memory !== undefined) {
      return [
        { value: `history:${memory.id}`, label: 'Open history', icon: 'clock' },
        { value: `delete:${memory.id}`, label: 'Delete memory', icon: 'trash' },
      ];
    }
    const scope = node.scopeId ?? '';
    const inThis = scopeKind(scope) === 'session' ? 'New memory in this session' : 'New memory in this scope';
    return [
      { value: `new-memory:${scope}`, label: inThis, icon: 'file-text' },
      ...newItems,
      // Only a scope with a file can be deleted; the implicit ones have none.
      ...(node.implicit === true
        ? []
        : [{ value: `delete-scope:${scope}`, label: 'Delete scope', icon: 'trash' }]),
    ];
  }

  private openMenu(event: MouseEvent, node: TreeNode | null): void {
    event.preventDefault();
    event.stopPropagation();
    this.menu = { x: event.clientX, y: event.clientY, items: this.itemsFor(node) };
  }

  private runMenu(value: string): void {
    this.menu = null;
    const [action, argument = ''] = value.split(/:(.*)/s);
    if (action === 'new-scope') navigate(paths.scopeNew());
    else if (action === 'new-memory') navigate(paths.memoryNew(argument));
    else if (action === 'history') navigate(paths.memoryHistory(argument));
    else if (action === 'delete') {
      requestDelete({ kind: 'memory', id: argument });
      navigate(paths.memory(argument));
    } else if (action === 'delete-scope') {
      requestDelete({ kind: 'scope', id: argument });
      navigate(paths.scope(argument));
    }
  }

  private renderNode(
    node: TreeNode,
    revealed: Set<string>,
    searching: boolean,
    selected: { memoryId: string; scopeId: string },
  ): TemplateResult {
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
      <span class="tree-row ${node.kind}" @contextmenu=${(event: MouseEvent) => this.openMenu(event, node)}>
        ${node.memory === undefined
          ? html`<sl-icon name=${nodeIcon(node)}></sl-icon>`
          : html`<fmn-kind-icon kind=${node.memory.kind}></fmn-kind-icon>`}
        <span class="tree-label" title=${node.memory?.description ?? node.label}>${node.label}</span>
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

  override updated(): void {
    // The menu is opened after it exists, so it can measure where to sit.
    if (this.menu === null) return;
    const dropdown = this.querySelector('.context-menu') as (HTMLElement & { open: boolean; show: () => void }) | null;
    if (dropdown !== null && !dropdown.open) dropdown.show();
  }

  private renderMenu(): TemplateResult | typeof nothing {
    const menu = this.menu;
    if (menu === null) return nothing;
    return html`<sl-dropdown
      class="context-menu"
      hoist
      placement="right-start"
      @sl-after-hide=${() => {
        this.menu = null;
      }}
    >
      <span slot="trigger" class="context-anchor" style="left: ${menu.x}px; top: ${menu.y}px"></span>
      <sl-menu
        @sl-select=${(event: CustomEvent<{ item: { value: string } }>) =>
          this.runMenu(event.detail.item.value)}
      >
        ${menu.items.map(
          (item) => html`<sl-menu-item value=${item.value}>
            <sl-icon slot="prefix" name=${item.icon}></sl-icon>${item.label}
          </sl-menu-item>`,
        )}
      </sl-menu>
    </sl-dropdown>`;
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
            <sl-dropdown class="new-menu" placement="bottom-end" hoist>
              <sl-icon-button slot="trigger" name="plus" label="New scope or memory"></sl-icon-button>
              <sl-menu
                @sl-select=${(event: CustomEvent<{ item: { value: string } }>) =>
                  this.runMenu(event.detail.item.value)}
              >
                ${newItems.map(
                  (item) => html`<sl-menu-item value=${item.value}>
                    <sl-icon slot="prefix" name=${item.icon}></sl-icon>${item.label}
                  </sl-menu-item>`,
                )}
              </sl-menu>
            </sl-dropdown>
          </div>
        </div>
        <div
          class="sidebar-tree"
          @contextmenu=${(event: MouseEvent) => this.openMenu(event, null)}
        >
          ${gate(this.tree.state, (nodes) => this.renderTree(nodes))}
        </div>
        ${this.renderMenu()}
      </div>
    `;
  }
}

customElements.define('fmn-sidebar', FmnSidebar);
