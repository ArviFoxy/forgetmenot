// The navigation hierarchy: every scope, with the memories that belong to it.
// A memory in several scopes appears under each of them.

import type { MemorySummary, ScopeDoc, ScopeType } from '../api/types';

/** A scope in the hierarchy, and the memories under it. */
export interface ScopeNode {
  id: string;
  /** `unknown` is a scope id a memory names that is neither a file nor implicit. */
  type: ScopeType | 'unknown';
  /** True when the scope has no file: `global`, `machine:<name>`, `session:<...>`. */
  implicit: boolean;
  memories: MemorySummary[];
}

/** The scope that is on in every context, so it is always part of the hierarchy. */
const globalScope = 'global';

function implicitType(id: string): ScopeType | 'unknown' {
  if (id === globalScope) return 'global';
  if (id.startsWith('machine:')) return 'machine';
  if (id.startsWith('session:')) return 'session';
  return 'unknown';
}

// Order on screen: the global scope, then the scopes with files, then the scopes
// that exist only because a memory or a session names them.
const groupRank: Record<ScopeType | 'unknown', number> = {
  global: 0,
  project: 1,
  domain: 1,
  directory: 1,
  machine: 2,
  session: 3,
  unknown: 4,
};

function byOrder(left: ScopeNode, right: ScopeNode): number {
  const leftRank = groupRank[left.type] + (left.implicit ? 0.5 : 0);
  const rightRank = groupRank[right.type] + (right.implicit ? 0.5 : 0);
  if (leftRank !== rightRank) return leftRank - rightRank;
  return left.id.localeCompare(right.id);
}

function byName(left: MemorySummary, right: MemorySummary): number {
  return left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
}

/**
 * Builds the hierarchy from the scope index, the memory index and the scopes the
 * live contexts have on. Scopes with a file are included even when no memory names
 * them; scopes without a file are included when a memory or a context names them,
 * and `global` always is.
 */
export function buildHierarchy(
  scopes: ScopeDoc[],
  memories: MemorySummary[],
  activeScopes: string[] = [],
): ScopeNode[] {
  const nodes = new Map<string, ScopeNode>();
  const node = (id: string): ScopeNode => {
    const found = nodes.get(id);
    if (found !== undefined) return found;
    const fresh: ScopeNode = { id, type: implicitType(id), implicit: true, memories: [] };
    nodes.set(id, fresh);
    return fresh;
  };

  node(globalScope);
  for (const scope of scopes) {
    const entry = node(scope.id);
    entry.type = scope.type;
    entry.implicit = false;
  }
  for (const memory of memories) {
    for (const scope of memory.scopes) node(scope).memories.push(memory);
  }
  for (const scope of activeScopes) node(scope);

  const ordered = [...nodes.values()].sort(byOrder);
  for (const entry of ordered) entry.memories.sort(byName);
  return ordered;
}

function matches(text: string, query: string): boolean {
  return text.toLowerCase().includes(query);
}

/**
 * The hierarchy a search text shows. Matching is case-insensitive and literal: the
 * text is compared as typed, with no fuzzy or pattern matching. A scope whose id
 * matches keeps all its memories; otherwise only its matching memories are kept and
 * a scope left with none is dropped.
 */
export function filterHierarchy(nodes: ScopeNode[], search: string): ScopeNode[] {
  const query = search.trim().toLowerCase();
  if (query === '') return nodes;
  const kept: ScopeNode[] = [];
  for (const node of nodes) {
    if (matches(node.id, query)) {
      kept.push(node);
      continue;
    }
    const memories = node.memories.filter(
      (memory) => matches(memory.name, query) || matches(memory.id, query) || matches(memory.title, query),
    );
    if (memories.length > 0) kept.push({ ...node, memories });
  }
  return kept;
}
