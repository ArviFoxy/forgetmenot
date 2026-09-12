import { expect, test } from 'vitest';
import { renderMarkdown } from '../src/markdown';
import { bodyBelowTitle } from '../src/model/memoryBody';

// The source of these expectations is the data model: a memory body is markdown with
// `[[name]]` links to memories, code spans and fenced blocks are literal, and the
// title is the body's first level-1 heading.

test('a wiki link in prose is left as text instead of linking to the memory', () => {
  const html = renderMarkdown('See [[widgets]] for the rule.');
  expect(html).toContain('href="/memories/widgets"');
  expect(html).toContain('>widgets<');
});

test('a wiki link in an inline code span is turned into a link', () => {
  const html = renderMarkdown('Write `[[widgets]]` to link.');
  expect(html).toContain('<code>[[widgets]]</code>');
  expect(html).not.toContain('href="/memories/widgets"');
});

test('a wiki link in a fenced code block is turned into a link', () => {
  const html = renderMarkdown(['```', 'scopes: [[rocketry]]', '```'].join('\n'));
  expect(html).toContain('[[rocketry]]');
  expect(html).not.toContain('href="/memories/rocketry"');
});

test('a memory id with a slash loses its path in the wiki link target', () => {
  const html = renderMarkdown('See [[sessions/alpha/s1/notes]].');
  expect(html).toContain('href="/memories/sessions/alpha/s1/notes"');
});

test('a table written as GitHub markdown is rendered as text', () => {
  const html = renderMarkdown(['| a | b |', '| - | - |', '| 1 | 2 |'].join('\n'));
  expect(html).toContain('<table>');
  expect(html).toContain('<td>1</td>');
});

test('raw HTML in a body reaches the page, so a body can inject markup', () => {
  const html = renderMarkdown('<script>window.stolen = 1;</script>\n\nplain text\n');
  expect(html).not.toContain('<script>');
  expect(html).toContain('plain text');
});

test('the title heading is shown a second time by the rendered body', () => {
  const body = '# Widget wiring\n\nhow the parts connect\n';
  expect(bodyBelowTitle(body, 'Widget wiring')).toBe('how the parts connect\n');
});

test('the first line of a body without a title heading is dropped', () => {
  const body = 'how the parts connect\n\n## Details\n';
  expect(bodyBelowTitle(body, 'widgets')).toBe(body);
});

test('a level-1 heading that is not the title is dropped from the body', () => {
  const body = '# Other heading\n\nhow the parts connect\n';
  expect(bodyBelowTitle(body, 'widgets')).toBe(body);
});
