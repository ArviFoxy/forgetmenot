import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import type { ValidationError } from '../api/types';
import { PageElement } from '../lib/element';

/** The validation errors the server returned for a write. */
export class FmnValidationErrors extends PageElement {
  static override properties: PropertyDeclarations = {
    errors: { attribute: false },
  };

  errors: ValidationError[] = [];

  override render(): TemplateResult | typeof nothing {
    if (this.errors.length === 0) return nothing;
    return html`<sl-alert variant="danger" open>
      <sl-icon slot="icon" name="exclamation-mark"></sl-icon>
      <strong>The write was refused</strong>
      <ul>
        ${this.errors.map((error) => html`<li><code>${error.path}</code> ${error.message}</li>`)}
      </ul>
    </sl-alert>`;
  }
}

customElements.define('fmn-validation-errors', FmnValidationErrors);
