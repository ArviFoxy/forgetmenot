// Memory bodies are markdown with `[[name]]` links to other memories. The pipeline
// is remark and rehype; raw HTML in a body is dropped, because rehype-stringify
// emits nothing for raw nodes unless it is asked to.

import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkRehype from 'remark-rehype';
import rehypeStringify from 'rehype-stringify';
import wikiLinkPlugin from 'remark-wiki-link';
import { paths } from './routes';

const processor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  .use(wikiLinkPlugin, {
    pageResolver: (name: string) => [name.trim()],
    hrefTemplate: (permalink: string) => paths.memory(permalink),
    wikiLinkClassName: 'wiki-link',
    newClassName: 'wiki-link',
  })
  .use(remarkRehype)
  .use(rehypeStringify)
  .freeze();

/** The HTML of a memory body, with every `[[name]]` an in-app link to that memory. */
export function renderMarkdown(text: string): string {
  return String(processor.processSync(text));
}
