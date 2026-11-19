import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

// A memory is a note, drawn as a sheet of paper. A pinned one is shown in full
// while its scope is active, and is drawn as a pin. Both are drawn in the same
// muted colour at the same size: the flag is a fact about the memory, not a warning.
export class FmnMemoryIcon extends PageElement {
  static override properties: PropertyDeclarations = {
    pinned: { type: Boolean },
  };

  pinned = false;

  override render(): TemplateResult {
    return this.pinned
      ? html`<sl-icon name="pin" label="pinned"></sl-icon>`
      : html`<sl-icon name="file-text" label="note"></sl-icon>`;
  }
}

customElements.define('fmn-memory-icon', FmnMemoryIcon);
