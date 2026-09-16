// The navigation tree: one node per scope the server lists, then the memories in
// each scope. A memory in several scopes appears under each of them.

import type { MemorySummary, ScopeRow } from '../api/types';

export type NodeKind = 'category' | 'scope' | 'memory';

export interface TreeNode {
  kind: NodeKind;
  /** Stable across rebuilds, so the expanded state survives a reload. */
  key: string;
  /** The text the tree shows. */
  label: string;
  /** The scope this node opens, for a scope node. */
  scopeId?: string;
  /** The row this node draws, for a scope node. */
  scope?: ScopeRow;
  /** The memory this node opens, for a memory node. */
  memory?: MemorySummary;
  children: TreeNode[];
  /** Memories below this node, counting one per place it appears. */
  count: number;
}

// The scopes are one list. A scope's kind is a label on the scope, not a place in
// the tree, so it does not group anything here. The one exception is the session
// scopes: there is one per session and they would bury the rest, so they sit under a
// "Sessions" node, by machine.
const sessionsCategory = { key: 'category:sessions', label: 'Sessions' };

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

function scopeNode(row: ScopeRow, memories: MemorySummary[], label: string): TreeNode {
  const children = [...memories]
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
    .map((memory) => memoryNode(row.id, memory));
  return {
    kind: 'scope',
    key: `scope:${row.id}`,
    label,
    scopeId: row.id,
    scope: row,
    children,
    count: children.length,
  };
}

/**
 * `session:<machine>/<id>` splits into the machine and the session. The only place
 * the frontend reads a scope id apart: what a scope is comes from its row, and this
 * is the one grouping the tree makes.
 */
function sessionParts(id: string): { machine: string; session: string } {
  const rest = id.slice('session:'.length);
  const slash = rest.indexOf('/');
  if (slash === -1) return { machine: rest, session: rest };
  return { machine: rest.slice(0, slash), session: rest.slice(slash + 1) };
}

/**
 * Sessions are two levels: the machine, then the sessions on it. A session its
 * context names is labelled with that name, which is what the user recognises; a
 * session with no name is labelled with the session part of its id.
 */
function sessionNodes(
  rows: ScopeRow[],
  memoriesOf: (id: string) => MemorySummary[],
): TreeNode[] {
  const machines = new Map<string, TreeNode[]>();
  for (const row of rows) {
    const { machine, session } = sessionParts(row.id);
    const name = row.name ?? '';
    const nodes = machines.get(machine) ?? [];
    nodes.push(scopeNode(row, memoriesOf(row.id), name === '' ? session : name));
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

/**
 * Builds the tree from the scope index and the memory index: one scope node per
 * row and nothing else, holding the memories whose `scopes` name that row. A
 * memory that names no listed row is in the tree nowhere.
 */
export function buildTree(rows: ScopeRow[], memories: MemorySummary[]): TreeNode[] {
  const held = new Map<string, MemorySummary[]>();
  for (const row of rows) held.set(row.id, []);
  for (const memory of memories) {
    for (const scope of memory.scopes) held.get(scope)?.push(memory);
  }
  const memoriesOf = (id: string): MemorySummary[] => held.get(id) ?? [];

  const flat: TreeNode[] = [];
  const sessions: ScopeRow[] = [];
  for (const row of rows) {
    if (row.kind === 'session') sessions.push(row);
    else flat.push(scopeNode(row, memoriesOf(row.id), row.id));
  }

  // The global scope leads, because it is on in every context; the rest follow by id.
  flat.sort((left, right) => {
    if (left.scope?.kind === 'global') return -1;
    if (right.scope?.kind === 'global') return 1;
    return byLabel(left, right);
  });

  if (sessions.length === 0) return flat;
  const children = sessionNodes(sessions, memoriesOf);
  return [
    ...flat,
    {
      kind: 'category',
      key: sessionsCategory.key,
      label: sessionsCategory.label,
      children,
      count: children.reduce((total, child) => total + child.count, 0),
    },
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
