import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { PageElement } from '../lib/element';
import { renderMarkdown } from '../markdown';

/** A memory body, rendered as a document. */
export class FmnMarkdown extends PageElement {
  static override properties: PropertyDeclarations = {
    text: { type: String },
  };

  text = '';

  override render(): TemplateResult {
    return html`<div class="markdown">${unsafeHTML(renderMarkdown(this.text))}</div>`;
  }
}

customElements.define('fmn-markdown', FmnMarkdown);
