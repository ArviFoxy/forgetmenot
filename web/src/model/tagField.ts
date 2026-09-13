// The rules a tag field follows, apart from the controls that show them.

import type { ContextRow, ScopeDoc } from '../api/types';
import { scopeKind } from './tree';

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
 * The scope ids worth offering: every scope with a file, `global`, the machine
 * scopes the live contexts have on, and any session scope the item already carries.
 * A session scope is one per session and is never offered on its own.
 */
export function suggestScopes(
  scopes: ScopeDoc[],
  contexts: ContextRow[],
  chosen: string[] = [],
): string[] {
  const offered = new Set<string>(['global']);
  for (const scope of scopes) offered.add(scope.id);
  for (const context of contexts) {
    for (const active of context.active_scopes) {
      if (scopeKind(active) === 'machine') offered.add(active);
    }
  }
  for (const scope of chosen) {
    if (scopeKind(scope) === 'session') offered.add(scope);
  }
  return [...offered].sort((left, right) => left.localeCompare(right));
}
