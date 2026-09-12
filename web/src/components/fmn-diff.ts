import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

function lineClass(line: string): string {
  if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ')) return 'diff-file';
  if (line.startsWith('@@')) return 'diff-hunk';
  if (line.startsWith('+')) return 'diff-add';
  if (line.startsWith('-')) return 'diff-del';
  return 'diff-context';
}

/** The server's unified diff text, one line kind per class. */
export class FmnDiff extends PageElement {
  static override properties: PropertyDeclarations = {
    diff: { type: String },
  };

  diff = '';

  override render(): TemplateResult {
    const lines = this.diff.replace(/\n$/, '').split('\n');
    return html`<pre class="diff">${lines.map(
      (line) => html`<code class=${lineClass(line)}>${line === '' ? ' ' : line}</code>`,
    )}</pre>`;
  }
}

customElements.define('fmn-diff', FmnDiff);
