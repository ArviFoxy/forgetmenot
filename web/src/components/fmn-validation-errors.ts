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
    return html`
      <table class="failure" role="alert">
        <thead>
          <tr>
            <th scope="col">Path</th>
            <th scope="col">Message</th>
          </tr>
        </thead>
        <tbody>
          ${this.errors.map(
            (error) => html`<tr>
              <td><code>${error.path}</code></td>
              <td>${error.message}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    `;
  }
}

customElements.define('fmn-validation-errors', FmnValidationErrors);
