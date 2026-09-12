import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import type { MemoryKind } from '../api/types';
import { PageElement } from '../lib/element';

// A critical memory is delivered in full and must not be missed, so it carries an
// exclamation mark; a knowledge memory is looked up, so it carries a book.
const iconClass: Record<MemoryKind, string> = {
  critical: 'bi-exclamation-circle-fill',
  knowledge: 'bi-book',
};

export class FmnKindIcon extends PageElement {
  static override properties: PropertyDeclarations = {
    kind: { type: String },
  };

  kind: MemoryKind = 'knowledge';

  override render(): TemplateResult {
    return html`<i
      class="bi ${iconClass[this.kind]} kind-${this.kind}"
      role="img"
      aria-label=${this.kind}
      title=${this.kind}
    ></i>`;
  }
}

customElements.define('fmn-kind-icon', FmnKindIcon);
