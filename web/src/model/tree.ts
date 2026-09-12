// The navigation tree: categories at the top, then scopes, then the memories in
// each scope. A memory in several scopes appears under each of them.

import type { MemorySummary, ScopeDoc, ScopeType } from '../api/types';

export type NodeKind = 'category' | 'scope' | 'memory';

export interface TreeNode {
  kind: NodeKind;
  /** Stable across rebuilds, so the expanded state survives a reload. */
  key: string;
  /** The text the tree shows. */
  label: string;
  /** The scope this node opens, for a scope node. */
  scopeId?: string;
  /** The memory this node opens, for a memory node. */
  memory?: MemorySummary;
  children: TreeNode[];
  /** Memories below this node, counting one per place it appears. */
  count: number;
}

/** The scope that is on in every context, so it is always in the tree. */
const globalScope = 'global';

/** A scope with no file: the id says what kind it is. */
function derivedType(id: string): ScopeType | 'unknown' {
  if (id === globalScope) return 'global';
  if (id.startsWith('machine:')) return 'machine';
  if (id.startsWith('session:')) return 'session';
  return 'unknown';
}

interface Category {
  key: string;
  label: string;
  /** Order among the top-level nodes. */
  order: number;
}

// The global scope has no category: it is one node at the top, because it is on
// everywhere and holds the memories that apply to everything.
const categories: Record<Exclude<ScopeType, 'global'> | 'unknown', Category> = {
  project: { key: 'projects', label: 'Projects', order: 1 },
  domain: { key: 'domains', label: 'Domains', order: 2 },
  directory: { key: 'directories', label: 'Directories', order: 3 },
  machine: { key: 'machines', label: 'Machines', order: 4 },
  session: { key: 'sessions', label: 'Sessions', order: 5 },
  unknown: { key: 'other', label: 'Other', order: 6 },
};

interface ScopeEntry {
  id: string;
  type: ScopeType | 'unknown';
  implicit: boolean;
  memories: MemorySummary[];
}

function byLabel(left: TreeNode, right: TreeNode): number {
  return left.label.localeCompare(right.label);
}

function memoryNode(scopeId: string, memory: MemorySummary): TreeNode {
  return {
    kind: 'memory',
    key: `memory:${scopeId}/${memory.id}`,
    label: memory.name,
    memory,
    children: [],
    count: 1,
  };
}

function scopeNode(entry: ScopeEntry, label: string): TreeNode {
  const children = [...entry.memories]
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
    .map((memory) => memoryNode(entry.id, memory));
  return {
    kind: 'scope',
    key: `scope:${entry.id}`,
    label,
    scopeId: entry.id,
    children,
    count: children.length,
  };
}

/** `session:<machine>/<id>` splits into the machine and the session. */
function sessionParts(id: string): { machine: string; session: string } {
  const rest = id.slice('session:'.length);
  const slash = rest.indexOf('/');
  if (slash === -1) return { machine: rest, session: rest };
  return { machine: rest.slice(0, slash), session: rest.slice(slash + 1) };
}

function categoryNode(category: Category, children: TreeNode[]): TreeNode {
  return {
    kind: 'category',
    key: `category:${category.key}`,
    label: category.label,
    children,
    count: children.reduce((total, child) => total + child.count, 0),
  };
}

/** Sessions are two levels: the machine, then the sessions on it. */
function sessionNodes(entries: ScopeEntry[]): TreeNode[] {
  const machines = new Map<string, TreeNode[]>();
  for (const entry of entries) {
    const { machine, session } = sessionParts(entry.id);
    const nodes = machines.get(machine) ?? [];
    nodes.push(scopeNode(entry, session));
    machines.set(machine, nodes);
  }
  return [...machines.entries()]
    .map(([machine, sessions]) => ({
      kind: 'category' as const,
      key: `sessions:${machine}`,
      label: machine,
      children: sessions.sort(byLabel),
      count: sessions.reduce((total, child) => total + child.count, 0),
    }))
    .sort(byLabel);
}

/** The label of a scope node: the part of the id its category does not already say. */
function scopeLabel(entry: ScopeEntry): string {
  if (entry.type === 'machine' && entry.id.startsWith('machine:')) {
    return entry.id.slice('machine:'.length);
  }
  return entry.id;
}

/**
 * Builds the tree from the scope index, the memory index and the scopes the live
 * contexts have on. Scopes with a file are included even when no memory names them;
 * scopes without a file are included when a memory or a context names them, and
 * `global` always is.
 */
export function buildTree(
  scopes: ScopeDoc[],
  memories: MemorySummary[],
  activeScopes: string[] = [],
): TreeNode[] {
  const entries = new Map<string, ScopeEntry>();
  const entry = (id: string): ScopeEntry => {
    const found = entries.get(id);
    if (found !== undefined) return found;
    const fresh: ScopeEntry = { id, type: derivedType(id), implicit: true, memories: [] };
    entries.set(id, fresh);
    return fresh;
  };

  entry(globalScope);
  for (const scope of scopes) {
    const found = entry(scope.id);
    found.type = scope.type;
    found.implicit = false;
  }
  for (const memory of memories) {
    for (const scope of memory.scopes) entry(scope).memories.push(memory);
  }
  for (const scope of activeScopes) entry(scope);

  const grouped = new Map<string, ScopeEntry[]>();
  const top: TreeNode[] = [];
  for (const found of [...entries.values()].sort((left, right) => left.id.localeCompare(right.id))) {
    if (found.type === 'global') {
      top.push(scopeNode(found, found.id));
      continue;
    }
    const category = categories[found.type];
    grouped.set(category.key, [...(grouped.get(category.key) ?? []), found]);
  }

  const categoryNodes: { order: number; node: TreeNode }[] = [];
  for (const [type, category] of Object.entries(categories)) {
    const members = grouped.get(category.key);
    if (members === undefined || members.length === 0) continue;
    const children =
      type === 'session'
        ? sessionNodes(members)
        : members.map((member) => scopeNode(member, scopeLabel(member))).sort(byLabel);
    categoryNodes.push({ order: category.order, node: categoryNode(category, children) });
  }

  return [
    ...top,
    ...categoryNodes.sort((left, right) => left.order - right.order).map((entry) => entry.node),
  ];
}

function matches(text: string, query: string): boolean {
  return text.toLowerCase().includes(query);
}

function nodeMatches(node: TreeNode, query: string): boolean {
  if (matches(node.label, query)) return true;
  if (node.scopeId !== undefined && matches(node.scopeId, query)) return true;
  const memory = node.memory;
  if (memory === undefined) return false;
  return matches(memory.id, query) || matches(memory.title, query);
}

function filterNode(node: TreeNode, query: string): TreeNode | null {
  if (nodeMatches(node, query)) return node;
  const children = node.children
    .map((child) => filterNode(child, query))
    .filter((child): child is TreeNode => child !== null);
  if (children.length === 0) return null;
  return {
    ...node,
    children,
    count: children.reduce((total, child) => total + (child.kind === 'memory' ? 1 : child.count), 0),
  };
}

/**
 * The tree a search text shows. Matching is case-insensitive and literal: the text
 * is compared as typed, with no fuzzy or pattern matching. A node that matches keeps
 * everything under it; a node that does not is kept only while something under it
 * matches.
 */
export function filterTree(nodes: TreeNode[], search: string): TreeNode[] {
  const query = search.trim().toLowerCase();
  if (query === '') return nodes;
  return nodes
    .map((node) => filterNode(node, query))
    .filter((node): node is TreeNode => node !== null);
}

/** The keys of every node that has to be open for `key` to be on screen. */
export function pathToKey(nodes: TreeNode[], key: string): string[] {
  for (const node of nodes) {
    if (node.key === key) return [node.key];
    const below = pathToKey(node.children, key);
    if (below.length > 0) return [node.key, ...below];
  }
  return [];
}

/** The keys of the nodes that hold the open memory or scope, and their ancestors. */
export function keysToReveal(nodes: TreeNode[], memoryId: string, scopeId: string): string[] {
  const revealed = new Set<string>();
  const walk = (node: TreeNode, ancestors: string[]): void => {
    const hit =
      (memoryId !== '' && node.memory?.id === memoryId) ||
      (scopeId !== '' && node.scopeId === scopeId);
    if (hit) for (const key of ancestors) revealed.add(key);
    for (const child of node.children) walk(child, [...ancestors, node.key]);
  };
  for (const node of nodes) walk(node, []);
  return [...revealed];
}
