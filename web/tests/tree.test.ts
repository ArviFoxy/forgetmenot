import { expect, test } from 'vitest';
import type { MemorySummary, ScopeRow } from '../src/api/types';
import { buildTree, filterTree, keysToReveal, type TreeNode } from '../src/model/tree';

// The source of these expectations is the data model in the README together with the
// shape the navigation promises: the server lists every scope that exists, one row
// each, and the tree draws one node per row holding the memories that name it, with
// the session rows gathered under a "Sessions" node by machine because there is one
// row per session.

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

/** The index row of a scope with a file. */
function fileRow(id: string): ScopeRow {
  return { id, kind: 'file', name: null, file: { id, message: null, implies: [], triggers: [], version: 'v' } };
}

const globalRow: ScopeRow = { id: 'global', kind: 'global', name: null, file: null };

function machineRow(machine: string): ScopeRow {
  return { id: `machine:${machine}`, kind: 'machine', name: null, file: null };
}

function sessionRow(machine: string, session: string, name: string | null = null): ScopeRow {
  return { id: `session:${machine}/${session}`, kind: 'session', name, file: null };
}

function find(nodes: TreeNode[], label: string): TreeNode {
  const found = findOrNull(nodes, label);
  if (found === null) throw new Error(`no node labelled ${label}`);
  return found;
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

/** The scope ids of every scope node in the tree, wherever it sits. */
function scopeIds(nodes: TreeNode[]): string[] {
  return nodes.flatMap((node) => [
    ...(node.kind === 'scope' ? [node.scopeId ?? ''] : []),
    ...scopeIds(node.children),
  ]);
}

test('a scope with a file is grouped instead of listed with the others', () => {
  const nodes = buildTree([globalRow, fileRow('widgets'), fileRow('rocketry'), fileRow('shed')], []);
  // One list, and nothing between the root and a scope.
  expect(labels(nodes)).toEqual(['global', 'rocketry', 'shed', 'widgets']);
});

test('the global scope is listed among the others instead of leading', () => {
  const nodes = buildTree([fileRow('alpha-project'), globalRow], [memory('bench-power', ['global'])]);
  expect(labels(nodes)[0]).toBe('global');
  expect(labels(find(nodes, 'global').children)).toEqual(['bench-power']);
});

test('a session memory is listed outside the Sessions category and its machine', () => {
  const nodes = buildTree(
    [sessionRow('alpha', 'session-1')],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const sessions = find(nodes, 'Sessions');
  expect(labels(sessions.children)).toEqual(['alpha']);
  const machine = find(sessions.children, 'alpha');
  expect(labels(machine.children)).toEqual(['session-1']);
  expect(labels(find(machine.children, 'session-1').children)).toEqual(['notes']);
});

test('a session the index names is listed by its id rather than by that name', () => {
  const nodes = buildTree([sessionRow('alpha', 'session-1', 'the thermocouple rig')], []);

  const listed = find(nodes, 'alpha').children;
  expect(listed).toHaveLength(1);
  expect(listed[0]?.scopeId).toBe('session:alpha/session-1');
  expect(listed[0]?.label).toBe('the thermocouple rig');
});

test('a machine scope is put under a category instead of in the list with the rest', () => {
  const nodes = buildTree([globalRow, machineRow('alpha'), fileRow('widgets')], []);
  expect(labels(nodes)).toEqual(['global', 'machine:alpha', 'widgets']);
});

test('a memory that names an id the index does not list is drawn as a scope of its own', () => {
  const nodes = buildTree([globalRow], [memory('stray', ['not-a-real-scope'])]);
  expect(labels(nodes)).toEqual(['global']);
  expect(find(nodes, 'global').children).toEqual([]);
  expect(findOrNull(nodes, 'stray')).toBeNull();
});

test('the tree drops a row the index lists, or draws a scope node for something else', () => {
  const rows = [
    globalRow,
    machineRow('alpha'),
    fileRow('widgets'),
    sessionRow('alpha', 'session-1'),
    sessionRow('beta', 'session-2', 'the bracket rework'),
  ];
  const drawn = scopeIds(buildTree(rows, [memory('widget-naming', ['widgets', 'global'])]));
  expect([...drawn].sort()).toEqual(rows.map((row) => row.id).sort());
});

test('the tree reads a context active scope, so an id no row lists can reach it', () => {
  // The rows and the memories are the whole input: there is no third parameter a
  // context's active set could come in through.
  expect(buildTree).toHaveLength(2);
});

test('a memory in two scopes appears under one of them only', () => {
  const nodes = buildTree(
    [fileRow('widgets'), fileRow('rocketry')],
    [memory('widget-naming', ['widgets', 'rocketry'])],
  );
  expect(labels(find(nodes, 'widgets').children)).toEqual(['widget-naming']);
  expect(labels(find(nodes, 'rocketry').children)).toEqual(['widget-naming']);
});

test('a count reports the node itself rather than the memories under it', () => {
  const nodes = buildTree(
    [fileRow('widgets'), sessionRow('alpha', 'session-1')],
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

// The filter is what the search box does: case-insensitive, literal, no pattern
// matching and no fuzzy matching.

const hierarchy = buildTree(
  [globalRow, fileRow('widgets'), fileRow('rocketry')],
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
    [sessionRow('alpha', 'session-1')],
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
    [sessionRow('alpha', 'session-1')],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const keys = keysToReveal(withSession, 'sessions/alpha/session-1/notes', '');
  expect(keys).toContain('category:sessions');
  expect(keys).toContain('sessions:alpha');
  expect(keys).toContain('scope:session:alpha/session-1');
});
