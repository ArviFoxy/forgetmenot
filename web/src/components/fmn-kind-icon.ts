import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import type { MemoryKind } from '../api/types';
import { PageElement } from '../lib/element';

// A critical memory is delivered in full, a knowledge memory is looked up. Both are
// ordinary kinds of memory, so both icons are drawn in the same muted colour at the
// same size: the kind is a fact about the memory, not a warning.
const iconName: Record<MemoryKind, string> = {
  critical: 'exclamation-mark',
  knowledge: 'book',
};

export class FmnKindIcon extends PageElement {
  static override properties: PropertyDeclarations = {
    kind: { type: String },
  };

  kind: MemoryKind = 'knowledge';

  override render(): TemplateResult {
    return html`<sl-icon name=${iconName[this.kind]} label=${this.kind}></sl-icon>`;
  }
}

customElements.define('fmn-kind-icon', FmnKindIcon);
