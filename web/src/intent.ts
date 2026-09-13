// One place for "the reader asked for this from somewhere else": the tree can ask a
// memory's page to open its delete panel, which is where the commit message for the
// deletion is typed.

let pendingDelete: string | null = null;

export function requestDelete(memoryId: string): void {
  pendingDelete = memoryId;
}

/** True once, for the page that the request named. */
export function takeDeleteIntent(memoryId: string): boolean {
  if (pendingDelete !== memoryId) return false;
  pendingDelete = null;
  return true;
}
