// One place for "the reader asked for this from somewhere else": the tree can ask an
// item's page to open its delete panel, which is where the commit message for the
// deletion is typed.

export type DeleteTarget = { kind: 'memory' | 'scope'; id: string };

let pending: DeleteTarget | null = null;

export function requestDelete(target: DeleteTarget): void {
  pending = target;
}

/** True once, for the page that the request named. */
export function takeDeleteIntent(kind: DeleteTarget['kind'], id: string): boolean {
  if (pending === null || pending.kind !== kind || pending.id !== id) return false;
  pending = null;
  return true;
}
