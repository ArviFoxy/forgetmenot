import { expect, test } from 'vitest';
import type { MemorySummary, ScopeDoc } from '../src/api/types';
import { buildTree, filterTree, keysToReveal, type TreeNode } from '../src/model/tree';

// The source of these expectations is the data model in the README together with the
// shape the navigation promises: categories at the top, the scopes of that kind
// below them, and the memories of a scope below it. `global` is always on and has no
// category; `machine:<name>` and `session:<machine>/<id>` are scopes with no file.

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
    archived: false,
    version: 'v',
    ...over,
  };
}

function scope(id: string, over: Partial<ScopeDoc> = {}): ScopeDoc {
  return { id, type: 'project', implies: [], triggers: [], version: 'v', ...over };
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

test('a scope with a file sits at the top level instead of under its category', () => {
  const nodes = buildTree(
    [scope('widgets'), scope('rocketry', { type: 'domain' }), scope('shed', { type: 'directory' })],
    [],
  );
  expect(labels(find(nodes, 'Projects').children)).toEqual(['widgets']);
  expect(labels(find(nodes, 'Domains').children)).toEqual(['rocketry']);
  expect(labels(find(nodes, 'Directories').children)).toEqual(['shed']);
  expect(labels(nodes)).not.toContain('widgets');
});

test('the global scope is put inside a category instead of at the top', () => {
  const nodes = buildTree([], [memory('bench-power', ['global'])]);
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

test('a machine scope a live context has on is dropped from Machines', () => {
  const nodes = buildTree([], [], ['machine:alpha', 'global']);
  expect(labels(find(nodes, 'Machines').children)).toEqual(['alpha']);
});

test('a memory in two scopes appears under one of them only', () => {
  const nodes = buildTree(
    [scope('widgets'), scope('rocketry', { type: 'domain' })],
    [memory('widget-naming', ['widgets', 'rocketry'])],
  );
  expect(labels(find(nodes, 'widgets').children)).toEqual(['widget-naming']);
  expect(labels(find(nodes, 'rocketry').children)).toEqual(['widget-naming']);
});

test('a count reports the node itself rather than the memories under it', () => {
  const nodes = buildTree(
    [scope('widgets'), scope('gadgets')],
    [memory('a', ['widgets']), memory('b', ['widgets']), memory('c', ['gadgets'])],
  );
  expect(find(nodes, 'widgets').count).toBe(2);
  expect(find(nodes, 'Projects').count).toBe(3);
});

test('a scope id a memory names that is neither a file nor implicit is dropped', () => {
  const nodes = buildTree([], [memory('stray', ['not-a-real-scope'])]);
  expect(labels(find(nodes, 'Other').children)).toEqual(['not-a-real-scope']);
});

// The filter is what the search box does: case-insensitive, literal, no pattern
// matching and no fuzzy matching.

const hierarchy = buildTree(
  [scope('widgets'), scope('rocketry', { type: 'domain' })],
  [
    memory('widget-naming', ['widgets']),
    memory('rocket-stages', ['rocketry']),
    memory('reading-list', ['global'], { title: 'Where the Workshop references live' }),
  ],
);

test('a search only matches when the case of the text matches', () => {
  expect(labels(filterTree(hierarchy, 'WIDGET'))).toEqual(['Projects']);
  expect(labels(find(filterTree(hierarchy, 'WIDGET'), 'Projects').children)).toEqual(['widgets']);
  const byTitle = filterTree(hierarchy, 'workshop references');
  expect(labels(byTitle)).toEqual(['global']);
  expect(labels(find(byTitle, 'global').children)).toEqual(['reading-list']);
});

test('a match drops the categories above it, so it cannot be found', () => {
  const shown = filterTree(hierarchy, 'rocket-stages');
  expect(labels(shown)).toEqual(['Domains']);
  expect(labels(find(shown, 'rocketry').children)).toEqual(['rocket-stages']);
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

test('the open memory is left buried, with its ancestors collapsed', () => {
  const keys = keysToReveal(hierarchy, 'rocket-stages', '');
  expect(keys).toContain('category:domains');
  expect(keys).toContain('scope:rocketry');
  expect(keys).not.toContain('category:projects');
});

test('the open scope is left buried, with its category collapsed', () => {
  const keys = keysToReveal(hierarchy, '', 'widgets');
  expect(keys).toContain('category:projects');
  expect(keys).not.toContain('scope:widgets');
});
