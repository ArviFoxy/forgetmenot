import { expect, test } from 'vitest';
import type { ContextRow } from '../src/api/types';
import { contextTree, type ContextNode } from '../src/model/contexts';

// The source of these expectations is what `/api/contexts` promises and what the
// page makes of it: a row's `parent` is the context that spawned it, which may be a
// subagent at any depth and is null for a main context; the rows arrive most
// recently seen first; and the page shows every row exactly once, each under the
// context that spawned it.

/** One context as the API answers it, with the parts this page reads. */
function context(over: Partial<ContextRow> & { key: string }): ContextRow {
  return {
    name: over.key,
    title: null,
    first_prompt: null,
    parent: null,
    task: null,
    agent_type: null,
    active_scopes: [],
    delivered_count: 0,
    last_seen: '2026-01-02T03:04:05+00:00',
    ...over,
  };
}

/** The keys of one level, in the order the table draws them. */
function keys(nodes: ContextNode[]): string[] {
  return nodes.map((node) => node.key);
}

/** The context of one key, wherever in the tree it ended up. */
function find(nodes: ContextNode[], key: string): ContextNode {
  for (const node of nodes) {
    if (node.key === key) return node;
    const below = find(node.children, key);
    if (below.key !== '') return below;
  }
  return { ...context({ key: '' }), children: [] };
}

/** Every key the tree holds, at every depth. */
function everyKey(nodes: ContextNode[]): string[] {
  return nodes.flatMap((node) => [node.key, ...everyKey(node.children)]);
}

/** The contexts below one, at any depth, which is the number its closed row stands for. */
function below(node: ContextNode): number {
  return node.children.reduce((count, child) => count + 1 + below(child), 0);
}

test('a subagent is left beside the session it runs in instead of under it', () => {
  const session = context({ key: 'alpha/session-1' });
  const agent = context({ key: 'alpha/session-1/agent-7f3a', parent: 'alpha/session-1' });

  const tree = contextTree([agent, session]);

  expect(keys(tree)).toEqual(['alpha/session-1']);
  expect(keys(tree[0]?.children ?? [])).toEqual(['alpha/session-1/agent-7f3a']);
});

test('a subagent of a subagent is lifted to the session, so its own parent reads as a leaf', () => {
  // The middle context spawned the deepest one; hanging the deepest one off the
  // session instead would make the page show a nesting that never happened.
  const session = context({ key: 'alpha/session-1' });
  const agent = context({ key: 'alpha/session-1/agent-7f3a', parent: 'alpha/session-1' });
  const deeper = context({
    key: 'alpha/session-1/agent-91b2',
    parent: 'alpha/session-1/agent-7f3a',
  });

  const tree = contextTree([deeper, agent, session]);

  expect(keys(tree)).toEqual(['alpha/session-1']);
  expect(keys(find(tree, 'alpha/session-1').children)).toEqual(['alpha/session-1/agent-7f3a']);
  expect(keys(find(tree, 'alpha/session-1/agent-7f3a').children)).toEqual([
    'alpha/session-1/agent-91b2',
  ]);
});

test('a context whose parent is not in the list is dropped from the page', () => {
  // The registry may hold a subagent whose session it has already forgotten.
  // Hanging it off nothing would lose the row; the page shows it as its own root.
  const orphan = context({ key: 'alpha/session-9/agent-4c1d', parent: 'alpha/session-9' });
  const session = context({ key: 'beta/session-1' });

  const tree = contextTree([orphan, session]);

  expect(keys(tree)).toEqual(['alpha/session-9/agent-4c1d', 'beta/session-1']);
  expect(everyKey(tree)).toHaveLength(2);
});

test('an ancestry that loops is followed forever, so the page never finishes', () => {
  // Two contexts naming each other, and a third naming one of them: none of the
  // three has an ancestry that ends, so each stands on its own.
  const first = context({ key: 'alpha/session-1', parent: 'alpha/session-2' });
  const second = context({ key: 'alpha/session-2', parent: 'alpha/session-1' });
  const hanging = context({ key: 'alpha/session-1/agent-7f3a', parent: 'alpha/session-1' });
  const itself = context({ key: 'beta/session-1', parent: 'beta/session-1' });

  const tree = contextTree([first, second, hanging, itself]);

  expect(everyKey(tree).sort()).toEqual(
    ['alpha/session-1', 'alpha/session-1/agent-7f3a', 'alpha/session-2', 'beta/session-1'].sort(),
  );
  expect(keys(tree)).toHaveLength(4);
});

test('a session stands where it was last seen itself, so a busy subagent buries it', () => {
  // The rows arrive most recently seen first. The subagent arrived first, which
  // is what puts its session ahead of a session seen after that session's own
  // last event; the deepest context does the same for every context above it.
  const deeper = context({
    key: 'alpha/session-1/agent-91b2',
    parent: 'alpha/session-1/agent-7f3a',
  });
  const quiet = context({ key: 'alpha/session-0' });
  const agent = context({ key: 'alpha/session-1/agent-7f3a', parent: 'alpha/session-1' });
  const session = context({ key: 'alpha/session-1' });

  const tree = contextTree([deeper, quiet, agent, session]);

  expect(keys(tree)).toEqual(['alpha/session-1', 'alpha/session-0']);
});

test('the contexts under one parent are reordered, so the one seen last is not the first', () => {
  const session = context({ key: 'alpha/session-1' });
  const busy = context({ key: 'alpha/session-1/agent-busy', parent: 'alpha/session-1' });
  const quiet = context({ key: 'alpha/session-1/agent-quiet', parent: 'alpha/session-1' });
  const deepest = context({
    key: 'alpha/session-1/agent-deep',
    parent: 'alpha/session-1/agent-quiet',
  });

  // The order they arrive in: the deepest context first, then the busy one, so
  // the quiet one is placed by the context under it rather than by itself.
  const tree = contextTree([deepest, busy, quiet, session]);

  expect(keys(find(tree, 'alpha/session-1').children)).toEqual([
    'alpha/session-1/agent-quiet',
    'alpha/session-1/agent-busy',
  ]);
});

test('a grandchild is missing from the subtree, so a closed row stands for too few rows', () => {
  const session = context({ key: 'alpha/session-1' });
  const agent = context({ key: 'alpha/session-1/agent-7f3a', parent: 'alpha/session-1' });
  const deeper = context({
    key: 'alpha/session-1/agent-91b2',
    parent: 'alpha/session-1/agent-7f3a',
  });
  const sibling = context({ key: 'alpha/session-1/agent-3ee0', parent: 'alpha/session-1' });
  const other = context({ key: 'beta/session-1' });

  const tree = contextTree([deeper, sibling, agent, session, other]);

  expect(below(find(tree, 'alpha/session-1'))).toBe(3);
  expect(below(find(tree, 'alpha/session-1/agent-7f3a'))).toBe(1);
  expect(below(find(tree, 'alpha/session-1/agent-91b2'))).toBe(0);
  expect(below(find(tree, 'beta/session-1'))).toBe(0);
});
