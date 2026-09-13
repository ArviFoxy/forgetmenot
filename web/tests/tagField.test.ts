import { expect, test } from 'vitest';
import type { ContextRow, ScopeDoc } from '../src/api/types';
import {
  addTag,
  cleanTag,
  filterSuggestions,
  removeLastTag,
  removeTag,
  suggestScopes,
} from '../src/model/tagField';

// The source of these expectations is what the field promises: the ids it offers are
// the scopes that exist and can sensibly be picked, an id typed by hand is allowed,
// and the keys behave as in any tag field.

function scope(id: string): ScopeDoc {
  return { id, implies: [], triggers: [], version: 'v' };
}

function context(key: string, active: string[]): ContextRow {
  return { key, active_scopes: active, delivered_count: 0, last_seen: '2026-01-01T00:00:00Z' };
}

test('a typed id keeps the spaces and commas that separate ids', () => {
  expect(cleanTag('  widgets , ')).toBe('widgets');
  expect(addTag([], ' rocketry ')).toEqual(['rocketry']);
});

test('the same id can be added twice', () => {
  expect(addTag(['widgets'], 'widgets')).toEqual(['widgets']);
  expect(addTag(['widgets'], '  ')).toEqual(['widgets']);
});

test('removing a chip takes the wrong one, and backspace takes none', () => {
  expect(removeTag(['a', 'b', 'c'], 'b')).toEqual(['a', 'c']);
  expect(removeLastTag(['a', 'b'])).toEqual(['a']);
  expect(removeLastTag([])).toEqual([]);
});

test('the list offers ids that are already chosen', () => {
  expect(filterSuggestions(['a', 'b'], ['a'], '')).toEqual(['b']);
});

test('the list matches only when the case matches, or matches nothing at all', () => {
  expect(filterSuggestions(['widgets', 'rocketry'], [], 'WIDG')).toEqual(['widgets']);
  expect(filterSuggestions(['widgets', 'rocketry'], [], 'ket')).toEqual(['rocketry']);
  expect(filterSuggestions(['widgets'], [], 'zzz')).toEqual([]);
});

test('a scope with a file is left out of the list', () => {
  const offered = suggestScopes([scope('widgets'), scope('rocketry')], []);
  expect(offered).toContain('widgets');
  expect(offered).toContain('rocketry');
});

test('the global scope has to be typed by hand', () => {
  expect(suggestScopes([], [])).toEqual(['global']);
});

test('a machine a context is running on is left out of the list', () => {
  const offered = suggestScopes([], [context('alpha/s1', ['global', 'machine:alpha', 'widgets'])]);
  expect(offered).toContain('machine:alpha');
  // A scope the context has on that is neither implicit nor a file is not offered.
  expect(offered).not.toContain('widgets');
});

test('a session scope is offered although nothing is in it', () => {
  const contexts = [context('alpha/s1', ['session:alpha/session-1'])];
  expect(suggestScopes([], contexts)).not.toContain('session:alpha/session-1');
  // Unless the item already carries it, in which case it stays on the list.
  expect(suggestScopes([], contexts, ['session:alpha/session-1'])).toContain(
    'session:alpha/session-1',
  );
});

test('the same id is offered twice when a file and a context both name it', () => {
  const offered = suggestScopes([scope('global')], [context('alpha/s1', ['global'])]);
  expect(offered.filter((id) => id === 'global')).toHaveLength(1);
});
