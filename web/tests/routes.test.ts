import { expect, test } from 'vitest';
import { paths, resolve } from '../src/routes';

// The source of these expectations is the app's own contract for addresses: the
// deep links `/memories/<id>` and `/scopes/<id>` must open the item, memory ids may
// contain slashes, and session scope ids contain a colon and a slash.

const memoryWithSlashes = 'sessions/alpha/session-1/notes';
const sessionScope = 'session:alpha/session-1';
const commitOid = 'a'.repeat(40);

/** Every link the app can show, with arguments that exercise the awkward ids. */
const links: Record<keyof typeof paths, string> = {
  home: paths.home(),
  memory: paths.memory(memoryWithSlashes),
  memoryEdit: paths.memoryEdit(memoryWithSlashes),
  memoryHistory: paths.memoryHistory(memoryWithSlashes),
  memoryCommit: paths.memoryCommit(memoryWithSlashes, commitOid),
  memoryNew: paths.memoryNew(),
  scope: paths.scope(sessionScope),
  scopeEdit: paths.scopeEdit(sessionScope),
  contexts: paths.contexts(),
  review: paths.review(),
  stats: paths.stats(),
  triggerTest: paths.triggerTest(),
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
  expect(view.properties.mode).toBe('view');
});

test('the edit address of a memory opens the document instead of the editor', () => {
  const view = resolve(paths.memoryEdit(memoryWithSlashes));
  expect(view.properties.memoryId).toBe(memoryWithSlashes);
  expect(view.properties.mode).toBe('edit');
});

test('the address of one commit loses the commit it names', () => {
  const view = resolve(paths.memoryCommit(memoryWithSlashes, commitOid));
  expect(view.properties.memoryId).toBe(memoryWithSlashes);
  expect(view.properties.mode).toBe('commit');
  expect(view.properties.oid).toBe(commitOid);
});

test('the address for creating a memory is read as a memory whose id is "new"', () => {
  const view = resolve(paths.memoryNew());
  expect(view.name).toBe('memoryNew');
  expect(view.properties.memoryId).toBeUndefined();
});

test('an address the app has no view for throws instead of resolving', () => {
  expect(resolve('/not/an/address').name).toBe('unknown');
  expect(resolve('/not/an/address').tag).toBe('fmn-unknown-view');
});
