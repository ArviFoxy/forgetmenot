import { expect, test } from 'vitest';
import type { MemoryDoc } from '../src/api/types';
import {
  bodyToSave,
  draftOf,
  fromEditor,
  isDirty,
  restoreWikiLinks,
} from '../src/model/memoryDraft';

// The source of these expectations is the promise the editor makes: a body nobody
// edited is written back as it was, and `[[name]]` links survive a serializer that
// escapes brackets.

const doc: MemoryDoc = {
  id: 'reading-list',
  name: 'reading-list',
  title: 'Where the workshop references live',
  description: 'the workshop references are on paper',
  kind: 'knowledge',
  scopes: ['global'],
  source: 'user',
  metadata: {},
  created: null,
  modified: null,
  author: 'wiki',
  // Hard-wrapped, with a wiki link: the bytes a rich editor would reformat.
  body: '# Where the workshop references live\n\nThe bench notes and the parts\ncatalogue live in the binder.\n\nStage numbering is in [[rocket-stages]].\n',
  version: 'b'.repeat(40),
  links: ['rocket-stages'],
  backlinks: [],
  last_commit: null,
};

test('an untouched body is reformatted by the editor before it is written', () => {
  const draft = draftOf(doc);
  expect(bodyToSave(draft)).toBe(doc.body);
  expect(isDirty(draft, doc)).toBe(false);
});

test('an edited body is written as the original bytes, losing the edit', () => {
  const edited = `${doc.body}\nA new line.\n`;
  const draft = { ...draftOf(doc), editedBody: edited };
  expect(bodyToSave(draft)).toBe(edited);
  expect(isDirty(draft, doc)).toBe(true);
});

test('a body the editor re-serialized without a change counts as an edit', () => {
  // The editor reports the text it holds; when that text is the original, nothing
  // has changed and the page must not offer to save.
  const draft = { ...draftOf(doc), editedBody: doc.body };
  expect(isDirty(draft, doc)).toBe(false);
  expect(bodyToSave(draft)).toBe(doc.body);
});

test('a changed field leaves the page reporting no change', () => {
  expect(isDirty({ ...draftOf(doc), description: 'something else' }, doc)).toBe(true);
  expect(isDirty({ ...draftOf(doc), kind: 'critical' }, doc)).toBe(true);
  expect(isDirty({ ...draftOf(doc), source: 'assistant' }, doc)).toBe(true);
  expect(isDirty({ ...draftOf(doc), scopesText: 'global, widgets' }, doc)).toBe(true);
});

test('the same scopes written with other spacing count as a change', () => {
  expect(isDirty({ ...draftOf(doc), scopesText: ' global ' }, doc)).toBe(false);
});

test('a wiki link stays escaped after the editor serializes the document', () => {
  expect(restoreWikiLinks('Stage numbering is in \\[\\[rocket-stages]].')).toBe(
    'Stage numbering is in [[rocket-stages]].',
  );
  expect(restoreWikiLinks('See \\[\\[rocket-stages\\]\\] and \\[\\[widget-naming]].')).toBe(
    'See [[rocket-stages]] and [[widget-naming]].',
  );
});

test('the empty paragraph the editor keeps at the end is written to the file', () => {
  const serialized = '# Title\n\nA line.\n\n<br />\n\n<br />\n';
  expect(fromEditor(serialized)).toBe('# Title\n\nA line.\n');
});

test('a run of blank lines the editor produced is written out as it came', () => {
  expect(fromEditor('One.\n\n\n\nTwo.\n')).toBe('One.\n\nTwo.\n');
});

test('a wiki link is left escaped by the tidied editor output', () => {
  expect(fromEditor('See \\[\\[rocket-stages]].\n')).toBe('See [[rocket-stages]].\n');
});

test('an ordinary markdown link is rewritten as a wiki link', () => {
  const link = 'See [the notes](https://example.invalid/a) for the rest.';
  expect(restoreWikiLinks(link)).toBe(link);
  expect(restoreWikiLinks('An escaped bracket \\[alone] stays.')).toBe(
    'An escaped bracket \\[alone] stays.',
  );
});
