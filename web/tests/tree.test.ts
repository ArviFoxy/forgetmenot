import { expect, test } from 'vitest';
import type { MemorySummary, ScopeDoc } from '../src/api/types';
import { buildTree, filterTree, keysToReveal, type TreeNode } from '../src/model/tree';

// The source of these expectations is the data model in the README together with the
// shape the navigation promises: one list of scopes, each holding its memories, with
// the session scopes gathered under a "Sessions" node by machine because there is one
// per session. A scope's type is a label on the scope and groups nothing. `global` is
// always on; `machine:<name>` and `session:<machine>/<id>` are scopes with no file.

function memory(id: string, scopes: string[], over: Partial<MemorySummary> = {}): MemorySummary {
  return {
    id,
    name: id.split('/').at(-1) ?? id,
    title: id,
    description: `description of ${id}`,
    kind: 'knowledge',
    scopes,
    source: 'user',
    modified: null,
    version: 'v',
    ...over,
  };
}

function scope(id: string, over: Partial<ScopeDoc> = {}): ScopeDoc {
  return { id, implies: [], triggers: [], version: 'v', ...over };
}

function find(nodes: TreeNode[], label: string): TreeNode {
  for (const node of nodes) {
    if (node.label === label) return node;
    const below = node.children.length > 0 ? findOrNull(node.children, label) : null;
    if (below !== null) return below;
  }
  throw new Error(`no node labelled ${label}`);
}

function findOrNull(nodes: TreeNode[], label: string): TreeNode | null {
  for (const node of nodes) {
    if (node.label === label) return node;
    const below = findOrNull(node.children, label);
    if (below !== null) return below;
  }
  return null;
}

function labels(nodes: TreeNode[]): string[] {
  return nodes.map((node) => node.label);
}

test('a scope with a file is grouped instead of listed with the others', () => {
  const nodes = buildTree([scope('widgets'), scope('rocketry'), scope('shed')], []);
  // One list, and nothing between the root and a scope.
  expect(labels(nodes)).toEqual(['global', 'rocketry', 'shed', 'widgets']);
});

test('the global scope is listed among the others instead of leading', () => {
  const nodes = buildTree([scope('alpha-project')], [memory('bench-power', ['global'])]);
  expect(labels(nodes)[0]).toBe('global');
  expect(labels(find(nodes, 'global').children)).toEqual(['bench-power']);
});

test('a session memory is listed outside the Sessions category and its machine', () => {
  const nodes = buildTree(
    [],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const sessions = find(nodes, 'Sessions');
  expect(labels(sessions.children)).toEqual(['alpha']);
  const machine = find(sessions.children, 'alpha');
  expect(labels(machine.children)).toEqual(['session-1']);
  expect(labels(find(machine.children, 'session-1').children)).toEqual(['notes']);
});

test('a machine scope is put under a category instead of in the list with the rest', () => {
  const nodes = buildTree([scope('widgets')], [], ['machine:alpha', 'global']);
  expect(labels(nodes)).toEqual(['global', 'machine:alpha', 'widgets']);
});

test('a scope with no file is reported as one that has a file, so the tree offers to delete it', () => {
  const nodes = buildTree([scope('widgets')], [], ['machine:alpha']);
  expect(find(nodes, 'widgets').implicit).toBe(false);
  expect(find(nodes, 'machine:alpha').implicit).toBe(true);
  expect(find(nodes, 'global').implicit).toBe(true);
});

test('a memory in two scopes appears under one of them only', () => {
  const nodes = buildTree(
    [scope('widgets'), scope('rocketry')],
    [memory('widget-naming', ['widgets', 'rocketry'])],
  );
  expect(labels(find(nodes, 'widgets').children)).toEqual(['widget-naming']);
  expect(labels(find(nodes, 'rocketry').children)).toEqual(['widget-naming']);
});

test('a count reports the node itself rather than the memories under it', () => {
  const nodes = buildTree(
    [scope('widgets')],
    [
      memory('a', ['widgets']),
      memory('b', ['widgets']),
      memory('notes', ['session:alpha/session-1']),
    ],
  );
  expect(find(nodes, 'widgets').count).toBe(2);
  expect(find(nodes, 'Sessions').count).toBe(1);
  expect(find(nodes, 'alpha').count).toBe(1);
});

test('a scope id a memory names that is neither a file nor implicit is dropped', () => {
  const nodes = buildTree([], [memory('stray', ['not-a-real-scope'])]);
  expect(labels(nodes)).toEqual(['global', 'not-a-real-scope']);
  expect(labels(find(nodes, 'not-a-real-scope').children)).toEqual(['stray']);
});

// The filter is what the search box does: case-insensitive, literal, no pattern
// matching and no fuzzy matching.

const hierarchy = buildTree(
  [scope('widgets'), scope('rocketry')],
  [
    memory('widget-naming', ['widgets']),
    memory('rocket-stages', ['rocketry']),
    memory('reading-list', ['global'], { title: 'Where the Workshop references live' }),
  ],
);

test('a search only matches when the case of the text matches', () => {
  expect(labels(filterTree(hierarchy, 'WIDGET'))).toEqual(['widgets']);
  const byTitle = filterTree(hierarchy, 'workshop references');
  expect(labels(byTitle)).toEqual(['global']);
  expect(labels(find(byTitle, 'global').children)).toEqual(['reading-list']);
});

test('a match drops the node above it, so it cannot be found', () => {
  const shown = filterTree(hierarchy, 'rocket-stages');
  expect(labels(shown)).toEqual(['rocketry']);
  expect(labels(find(shown, 'rocketry').children)).toEqual(['rocket-stages']);
});

test('a session memory that matches loses the Sessions node above it', () => {
  const withSession = buildTree(
    [],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const shown = filterTree(withSession, 'notes');
  expect(labels(shown)).toEqual(['Sessions']);
  expect(labels(find(shown, 'alpha').children)).toEqual(['session-1']);
});

test('a scope that matches keeps only the memories that match', () => {
  const shown = filterTree(hierarchy, 'widgets');
  expect(labels(find(shown, 'widgets').children)).toEqual(['widget-naming']);
});

test('an empty search hides the tree instead of showing all of it', () => {
  expect(filterTree(hierarchy, '')).toBe(hierarchy);
  expect(filterTree(hierarchy, '   ')).toBe(hierarchy);
});

test('a search that matches nothing keeps branches on screen', () => {
  expect(filterTree(hierarchy, 'zzz')).toEqual([]);
});

test('the open memory is left buried, with the scope above it collapsed', () => {
  const keys = keysToReveal(hierarchy, 'rocket-stages', '');
  expect(keys).toContain('scope:rocketry');
  expect(keys).not.toContain('scope:widgets');
});

test('a session memory is left buried, with Sessions and its machine collapsed', () => {
  const withSession = buildTree(
    [],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const keys = keysToReveal(withSession, 'sessions/alpha/session-1/notes', '');
  expect(keys).toContain('category:sessions');
  expect(keys).toContain('sessions:alpha');
  expect(keys).toContain('scope:session:alpha/session-1');
});
