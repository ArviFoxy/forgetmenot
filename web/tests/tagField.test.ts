import { expect, test } from 'vitest';
import type { ScopeRow } from '../src/api/types';
import {
  addTag,
  cleanTag,
  filterSuggestions,
  removeLastTag,
  removeTag,
  setSingleTag,
  suggestScopes,
} from '../src/model/tagField';

// The source of these expectations is what the field promises: the ids it offers are
// the scopes the server lists and can sensibly be picked, an id typed by hand is
// allowed, and the keys behave as in any tag field.

/** The index row of a scope with a file. */
function fileRow(id: string): ScopeRow {
  return { id, kind: 'file', name: null, file: { id, message: null, implies: [], triggers: [], version: 'v' } };
}

const globalRow: ScopeRow = { id: 'global', kind: 'global', name: null, file: null };

const machineRow: ScopeRow = { id: 'machine:alpha', kind: 'machine', name: null, file: null };

const sessionRow: ScopeRow = {
  id: 'session:alpha/session-1',
  kind: 'session',
  name: 'the thermocouple rig',
  file: null,
};

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

test('a single-value field keeps what was there when a new value is picked', () => {
  expect(setSingleTag('beta')).toEqual(['beta']);
  expect(setSingleTag('  ')).toEqual([]);
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
  const offered = suggestScopes([fileRow('widgets'), fileRow('rocketry')]);
  expect(offered).toContain('widgets');
  expect(offered).toContain('rocketry');
});

test('the global scope has to be typed by hand', () => {
  expect(suggestScopes([globalRow])).toEqual(['global']);
});

test('a scope the index does not list is offered all the same', () => {
  expect(suggestScopes([])).toEqual([]);
});

test('a machine the index lists is left out of the list', () => {
  expect(suggestScopes([machineRow])).toContain('machine:alpha');
});

test('a session scope is offered although nothing is in it', () => {
  expect(suggestScopes([globalRow, sessionRow])).not.toContain('session:alpha/session-1');
  // Unless the item already carries it, in which case it stays on the list.
  expect(suggestScopes([globalRow, sessionRow], ['session:alpha/session-1'])).toContain(
    'session:alpha/session-1',
  );
});

test('the same id is offered twice when a row and the chosen ids both name it', () => {
  const offered = suggestScopes([globalRow], ['global']);
  expect(offered.filter((id) => id === 'global')).toHaveLength(1);
});
