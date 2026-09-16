// The rules a tag field follows, apart from the controls that show them.

import type { ScopeRow } from '../api/types';

/** An id is one token: no commas, no surrounding space. */
export function cleanTag(text: string): string {
  return text.replace(/,/g, '').trim();
}

/** The chosen ids with `text` added, unless it is empty or already there. */
export function addTag(chosen: string[], text: string): string[] {
  const tag = cleanTag(text);
  if (tag === '' || chosen.includes(tag)) return chosen;
  return [...chosen, tag];
}

/**
 * The one chosen id for a field that holds a single name: what is typed replaces
 * what was there, and an empty text clears it.
 */
export function setSingleTag(text: string): string[] {
  const tag = cleanTag(text);
  return tag === '' ? [] : [tag];
}

export function removeTag(chosen: string[], tag: string): string[] {
  return chosen.filter((entry) => entry !== tag);
}

/** Backspace in an empty field takes the last chip back. */
export function removeLastTag(chosen: string[]): string[] {
  return chosen.slice(0, -1);
}

/**
 * What the list under the field offers: the ids that are not chosen yet and that
 * contain the typed text, compared case-insensitively and literally.
 */
export function filterSuggestions(all: string[], chosen: string[], text: string): string[] {
  const query = cleanTag(text).toLowerCase();
  return all
    .filter((id) => !chosen.includes(id))
    .filter((id) => query === '' || id.toLowerCase().includes(query));
}

/**
 * The scope ids worth offering: the scopes with a file, `global` and the machines,
 * as the index lists them, together with the ids the item already carries. There is
 * one session scope per session, so a session is offered only when it is carried.
 */
export function suggestScopes(rows: ScopeRow[], chosen: string[] = []): string[] {
  const offered = new Set<string>();
  for (const row of rows) {
    if (row.kind !== 'session') offered.add(row.id);
  }
  for (const scope of chosen) offered.add(scope);
  return [...offered].sort((left, right) => left.localeCompare(right));
}
