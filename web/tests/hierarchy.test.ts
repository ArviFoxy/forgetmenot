import { expect, test } from 'vitest';
import type { MemorySummary, ScopeDoc } from '../src/api/types';
import { buildHierarchy, filterHierarchy } from '../src/model/hierarchy';

// The source of these expectations is the data model in the README: a memory belongs
// to one or more scopes, `global` is always on, and `machine:<name>` and
// `session:<machine>/<id>` are scopes with no file.

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

function nodeOf(nodes: ReturnType<typeof buildHierarchy>, id: string) {
  const found = nodes.find((node) => node.id === id);
  if (found === undefined) throw new Error(`no scope ${id} in the hierarchy`);
  return found;
}

test('a memory in two scopes appears under one of them only', () => {
  const nodes = buildHierarchy(
    [scope('widgets'), scope('rocketry', { type: 'domain' })],
    [memory('widget-naming', ['widgets', 'rocketry'])],
  );
  expect(nodeOf(nodes, 'widgets').memories.map((entry) => entry.id)).toEqual(['widget-naming']);
  expect(nodeOf(nodes, 'rocketry').memories.map((entry) => entry.id)).toEqual(['widget-naming']);
});

test('a scope with a file and no memories is dropped from the hierarchy', () => {
  const nodes = buildHierarchy([scope('workshop', { type: 'directory' })], []);
  expect(nodeOf(nodes, 'workshop').memories).toEqual([]);
  expect(nodeOf(nodes, 'workshop').implicit).toBe(false);
  expect(nodeOf(nodes, 'workshop').type).toBe('directory');
});

test('a session scope a memory names is dropped because it has no file', () => {
  const nodes = buildHierarchy(
    [],
    [memory('sessions/alpha/session-1/notes', ['session:alpha/session-1'])],
  );
  const node = nodeOf(nodes, 'session:alpha/session-1');
  expect(node.implicit).toBe(true);
  expect(node.type).toBe('session');
  expect(node.memories.map((entry) => entry.id)).toEqual(['sessions/alpha/session-1/notes']);
});

test('the global scope is missing when no memory and no context names it', () => {
  const nodes = buildHierarchy([], []);
  expect(nodeOf(nodes, 'global').type).toBe('global');
  expect(nodeOf(nodes, 'global').implicit).toBe(true);
});

test('a machine scope that only a live context has on is dropped', () => {
  const nodes = buildHierarchy([], [], ['machine:alpha', 'global']);
  expect(nodeOf(nodes, 'machine:alpha').type).toBe('machine');
  expect(nodeOf(nodes, 'machine:alpha').memories).toEqual([]);
});

test('a scope with a file is reported as one without a file', () => {
  const nodes = buildHierarchy([scope('widgets')], [memory('widget-naming', ['widgets'])]);
  expect(nodeOf(nodes, 'widgets').implicit).toBe(false);
  expect(nodeOf(nodes, 'widgets').type).toBe('project');
});

// The filter is what the search box does: case-insensitive, literal, no pattern
// matching and no fuzzy matching.

const hierarchy = buildHierarchy(
  [scope('widgets'), scope('rocketry', { type: 'domain' })],
  [
    memory('widget-naming', ['widgets']),
    memory('rocket-stages', ['rocketry']),
    memory('reading-list', ['global'], { title: 'Where the Workshop references live' }),
  ],
);

test('a search only matches when the case of the text matches', () => {
  // The query is upper case and the scope id lower case.
  expect(filterHierarchy(hierarchy, 'WIDGET').map((node) => node.id)).toEqual(['widgets']);
  // The query is lower case and the memory's title has capitals.
  const byTitle = filterHierarchy(hierarchy, 'workshop references');
  expect(byTitle.map((node) => node.id)).toEqual(['global']);
  expect(byTitle[0]?.memories.map((entry) => entry.id)).toEqual(['reading-list']);
});

test('a search that matches a scope id hides the memories under that scope', () => {
  const shown = filterHierarchy(hierarchy, 'rocketry');
  expect(nodeOf(shown, 'rocketry').memories.map((entry) => entry.id)).toEqual(['rocket-stages']);
});

test('a scope keeps memories that do not match the search', () => {
  const shown = filterHierarchy(hierarchy, 'rocket-stages');
  expect(shown.map((node) => node.id)).toEqual(['rocketry']);
  expect(nodeOf(shown, 'rocketry').memories.map((entry) => entry.id)).toEqual(['rocket-stages']);
});

test('a search matches text that is not in the name, the id or the title', () => {
  // "description of" appears in every description and in nothing that is shown.
  expect(filterHierarchy(hierarchy, 'description of')).toEqual([]);
});

test('an empty search hides the hierarchy instead of showing all of it', () => {
  expect(filterHierarchy(hierarchy, '')).toBe(hierarchy);
  expect(filterHierarchy(hierarchy, '   ')).toBe(hierarchy);
});

test('a search that matches nothing keeps scopes on screen', () => {
  expect(filterHierarchy(hierarchy, 'zzz')).toEqual([]);
});
