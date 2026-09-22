// The contexts page's rows: the flat list `/api/contexts` answers, nested by the
// context each row was spawned from, and what each column of the table holds.

import { html } from 'lit';
import type { ContextRow } from '../api/types';
import { paths } from '../routes';
import type { TableColumn } from './stats';

/** One context with the contexts spawned inside it, at any depth. */
export interface ContextNode extends ContextRow {
  children: ContextNode[];
}

/** The machine part of a context key, which is everything before the first `/`. */
function machineOf(key: string): string {
  const slash = key.indexOf('/');
  return slash === -1 ? key : key.slice(0, slash);
}

/**
 * The rest of a context key: `<session-id>`, or `<session-id>/<agent-id>` for a
 * subagent, at any depth.
 */
function idOf(key: string): string {
  const slash = key.indexOf('/');
  return slash === -1 ? '' : key.slice(slash + 1);
}

/**
 * What the Name cell reads: the name the server derived, or the context's own id
 * when nothing but the key is known about it, so that every row has text to open
 * the context's prompt by.
 */
function nameOf(row: ContextRow): string {
  return row.name === '' ? idOf(row.key) : row.name;
}

/**
 * The context a row hangs under: the one its `parent` names. Null when the row
 * is a root, which it is when it names no parent, when the parent is not in the
 * list, and when walking up from it comes back to a context already passed.
 */
function parentOf(node: ContextNode, nodes: Map<string, ContextNode>): ContextNode | null {
  if (node.parent === null) return null;
  const parent = nodes.get(node.parent);
  if (parent === undefined) return null;
  const passed = new Set<string>([node.key]);
  let above: ContextNode | undefined = parent;
  while (above !== undefined) {
    if (passed.has(above.key)) return null;
    passed.add(above.key);
    above = above.parent === null ? undefined : nodes.get(above.parent);
  }
  return parent;
}

/**
 * Puts the contexts under `node` in the order the page shows them, at every
 * depth, and answers where `node` itself stands.
 *
 * `rank` is the place a context arrived at. A context stands at the place of the
 * most recently seen context of its own subtree, so a session sits where its
 * busiest subagent puts it however deep that subagent runs.
 */
function arrange(node: ContextNode, rank: Map<string, number>): number {
  const places = new Map(node.children.map((child) => [child.key, arrange(child, rank)]));
  node.children.sort((left, right) => (places.get(left.key) ?? 0) - (places.get(right.key) ?? 0));
  return Math.min(rank.get(node.key) ?? 0, ...places.values());
}

/**
 * The contexts as a tree. Every row is in it exactly once: under the context
 * that spawned it when that context is in the list too, and as a root otherwise.
 *
 * The rows arrive most recently seen first and keep that order among the
 * contexts of one parent, with a parent standing where the most recently seen
 * context below it stands.
 */
export function contextTree(rows: ContextRow[]): ContextNode[] {
  const nodes = new Map<string, ContextNode>();
  for (const row of rows) nodes.set(row.key, { ...row, children: [] });
  const arrived = [...nodes.values()];
  const rank = new Map(arrived.map((node, place) => [node.key, place]));
  const roots: ContextNode[] = [];
  for (const node of arrived) {
    const parent = parentOf(node, nodes);
    if (parent === null) roots.push(node);
    else parent.children.push(node);
  }
  const places = new Map(roots.map((root) => [root.key, arrange(root, rank)]));
  roots.sort((left, right) => (places.get(left.key) ?? 0) - (places.get(right.key) ?? 0));
  return roots;
}

/**
 * What the contexts table shows. On a narrow screen the row keeps what names it
 * and when it was last seen; the machine and the delivered count go first, then
 * the scopes and the id, which the row lists inside itself once opened.
 */
export const contextColumns: TableColumn<ContextNode>[] = [
  {
    id: 'name',
    header: 'Name',
    value: (row) => nameOf(row),
    link: (row) => paths.contextPrompt(row.key),
    wraps: true,
  },
  {
    id: 'scopes',
    header: 'Scopes',
    value: (row) => [...row.active_scopes].sort().join(' '),
    cell: (row) => html`<span class="chips"
      >${[...row.active_scopes].sort().map(
        (scope) =>
          html`<a href=${paths.scope(scope)}
            ><sl-badge variant="neutral" pill>${scope}</sl-badge></a
          >`,
      )}</span
    >`,
    wraps: true,
    priority: 2,
  },
  { id: 'id', header: 'Id', value: (row) => idOf(row.key), mono: true, priority: 2 },
  { id: 'machine', header: 'Machine', value: (row) => machineOf(row.key), priority: 3 },
  {
    id: 'delivered',
    header: 'Delivered',
    value: (row) => row.delivered_count,
    numeric: true,
    priority: 3,
  },
  { id: 'last-seen', header: 'Last seen', value: (row) => row.last_seen, moment: true },
];
