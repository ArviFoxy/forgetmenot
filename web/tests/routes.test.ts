import { expect, test } from 'vitest';
import { paths, promptModeFromSearch, resolve, scopeFromSearch } from '../src/routes';

// The source of these expectations is the app's own contract for addresses: the
// deep links `/memories/<id>` and `/scopes/<id>` must open the item, memory ids may
// contain slashes, and session scope ids contain a colon and a slash.

const memoryWithSlashes = 'sessions/alpha/session-1/notes';
const sessionScope = 'session:alpha/session-1';
const subagentKey = 'alpha/session-1/agent-7f3a';
const commitOid = 'a'.repeat(40);

/** Every link the app can show, with arguments that exercise the awkward ids. */
const links: Record<keyof typeof paths, string> = {
  home: paths.home(),
  memory: paths.memory(memoryWithSlashes),
  memoryHistory: paths.memoryHistory(memoryWithSlashes),
  memoryCommit: paths.memoryCommit(memoryWithSlashes, commitOid),
  memoryNew: paths.memoryNew(),
  scopeNew: paths.scopeNew(),
  scope: paths.scope(sessionScope),
  history: paths.history(),
  historyCommit: paths.historyCommit(commitOid),
  contexts: paths.contexts(),
  contextPrompt: paths.contextPrompt(subagentKey),
  stats: paths.stats(),
  settings: paths.settings(),
};

test('a link the app shows opens no view when its address is loaded directly', () => {
  for (const [name, path] of Object.entries(links)) {
    const view = resolve(path);
    expect(view.name, `${name} -> ${path}`).not.toBe('unknown');
    expect(view.tag, `${name} -> ${path}`).toMatch(/^fmn-[a-z-]+$/);
  }
});

test('a link builder is added without a route to match it', () => {
  // The table above is the coverage claim: a new builder with no sample here, or a
  // sample whose address no route matches, fails this.
  expect(Object.keys(links).sort()).toEqual(Object.keys(paths).sort());
  for (const path of Object.values(links)) {
    expect(resolve(path).name, path).not.toBe('unknown');
  }
});

test('a memory id loses its slashes between the link and the view', () => {
  const view = resolve(paths.memory(memoryWithSlashes));
  expect(view.properties.memoryId).toBe(memoryWithSlashes);
  expect(view.properties.mode).toBe('document');
});

test('a scope id loses its colon or its slash between the link and the view', () => {
  const view = resolve(paths.scope(sessionScope));
  expect(view.properties.scopeId).toBe(sessionScope);
  expect(view.name).toBe('scope');
});

test('a context key loses its slashes between the link and the prompt page', () => {
  const view = resolve(paths.contextPrompt(subagentKey));
  expect(view.name).toBe('contextPrompt');
  expect(view.properties.contextKey).toBe(subagentKey);
});

test('the mode a prompt address carries is dropped, so both modes open the same text', () => {
  const search = (path: string): string => new URL(path, 'http://localhost').search;
  expect(promptModeFromSearch(search(paths.contextPrompt(subagentKey, 'all')))).toBe('all');
  expect(promptModeFromSearch(search(paths.contextPrompt(subagentKey, 'due')))).toBe('due');
  expect(promptModeFromSearch(search(paths.contextPrompt(subagentKey)))).toBe('due');
});

test('a separate edit address still exists, so an item has two pages', () => {
  // There is one page per item: /memories/<id>/edit is not a route, so it reads as a
  // memory whose id ends in "edit".
  const view = resolve('/memories/widget-naming/edit');
  expect(view.properties.mode).toBe('document');
  expect(view.properties.memoryId).toBe('widget-naming/edit');
  expect(resolve('/scopes/widgets/edit').properties.scopeId).toBe('widgets/edit');
});

test('the address of one commit loses the commit it names', () => {
  const view = resolve(paths.memoryCommit(memoryWithSlashes, commitOid));
  expect(view.properties.memoryId).toBe(memoryWithSlashes);
  expect(view.properties.mode).toBe('commit');
  expect(view.properties.oid).toBe(commitOid);
});

test('the address of one commit of the store loses the commit it names', () => {
  const view = resolve(paths.historyCommit(commitOid));
  expect(view.name).toBe('historyCommit');
  expect(view.properties.oid).toBe(commitOid);
  // The same view as the list, told to show one commit rather than all of them.
  expect(view.tag).toBe(resolve(paths.history()).tag);
  expect(view.properties.mode).toBe('commit');
  expect(resolve(paths.history()).properties.mode).toBe('list');
});

test('the address for creating a memory is read as a memory whose id is "new"', () => {
  const view = resolve(paths.memoryNew());
  expect(view.name).toBe('memoryNew');
  expect(view.properties.memoryId).toBeUndefined();
  // The same view as a memory that exists, told to start from nothing.
  expect(view.tag).toBe('fmn-memory-page');
  expect(view.properties.mode).toBe('new');
});

test('the address for creating a scope is read as a scope whose id is "new"', () => {
  const view = resolve(paths.scopeNew());
  expect(view.name).toBe('scopeNew');
  expect(view.properties.scopeId).toBeUndefined();
  expect(view.tag).toBe('fmn-scope-page');
  expect(view.properties.mode).toBe('new');
});

test('the scope a new memory starts in is lost between the link and the page', () => {
  const link = paths.memoryNew('session:alpha/session-1');
  expect(resolve(link.split('?')[0] ?? '').name).toBe('memoryNew');
  expect(scopeFromSearch(new URL(link, 'http://x').search)).toBe('session:alpha/session-1');
  expect(scopeFromSearch('')).toBe('');
});

test('an address the app has no view for throws instead of resolving', () => {
  expect(resolve('/not/an/address').name).toBe('unknown');
  expect(resolve('/not/an/address').tag).toBe('fmn-unknown-view');
});
