// The edit in progress on one memory, and the two rules its save follows:
// an untouched body is written back byte for byte, and `[[name]]` links survive the
// editor's serializer.

import type { MemoryDoc, MemoryKind, MemorySource, ValidationError } from '../api/types';

export interface MemoryDraft {
  /** The version the edit started from, and the body as it was loaded. */
  baseVersion: string;
  originalBody: string;
  description: string;
  kind: MemoryKind;
  /** The one scope the memory belongs to, empty while a new memory has none. */
  scope: string;
  source: MemorySource;
  /** The markdown the editor holds, once the reader has edited it. */
  editedBody: string | null;
  message: string;
  saving: boolean;
  errors: ValidationError[];
  conflict: MemoryDoc | null;
  failure: string | null;
}

export function draftOf(doc: MemoryDoc): MemoryDraft {
  return {
    baseVersion: doc.version,
    originalBody: doc.body,
    description: doc.description,
    kind: doc.kind,
    scope: doc.scope,
    source: doc.source,
    editedBody: null,
    message: '',
    saving: false,
    errors: [],
    conflict: null,
    failure: null,
  };
}

/**
 * A markdown serializer escapes a leading `[` because it could start a link, which
 * would turn a wiki link into `\[\[name]]` and break it. The escapes are taken back
 * out here, so a body that mentions `[[name]]` keeps mentioning it.
 */
export function restoreWikiLinks(markdown: string): string {
  return markdown.replace(/\\\[\\\[([^\n[\]]+?)\\?\]\\?\]/g, '[[$1]]');
}

/**
 * The markdown the editor holds, tidied: the editor keeps an empty paragraph at the
 * end of the document to click into, and that serializes as `<br />`, which is not
 * something anybody typed. Runs of blank lines are collapsed for the same reason.
 */
export function fromEditor(markdown: string): string {
  const withoutBreaks = restoreWikiLinks(markdown)
    .split('\n')
    .filter((line) => line.trim() !== '<br />')
    .join('\n');
  return `${withoutBreaks.replace(/\n{3,}/g, '\n\n').trimEnd()}\n`;
}

/**
 * The body to write. An untouched body is the bytes that were loaded, so saving a
 * memory whose text nobody changed produces no diff; an edited body is what the
 * editor serialized, which may be formatted differently from the original.
 */
export function bodyToSave(draft: MemoryDraft): string {
  return draft.editedBody ?? draft.originalBody;
}

/** True when something on the page differs from the loaded memory. */
export function isDirty(draft: MemoryDraft, doc: MemoryDoc): boolean {
  if (draft.editedBody !== null && draft.editedBody !== draft.originalBody) return true;
  if (draft.description !== doc.description) return true;
  if (draft.kind !== doc.kind) return true;
  if (draft.source !== doc.source) return true;
  return draft.scope !== doc.scope;
}

/** The fields and the body as one text, so the two sides of a conflict compare. */
export function memoryText(fields: {
  description: string;
  kind: string;
  scope: string;
  source: string;
  body: string;
}): string {
  return [
    `description: ${fields.description}`,
    `kind: ${fields.kind}`,
    `scope: ${fields.scope}`,
    `source: ${fields.source}`,
    '',
    fields.body,
  ].join('\n');
}
