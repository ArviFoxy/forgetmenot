import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

/** The commit message every write needs, with the save button next to it. */
export class FmnCommitBar extends PageElement {
  static override properties: PropertyDeclarations = {
    message: { type: String },
    saving: { type: Boolean },
    saveLabel: { type: String },
    cancelHref: { type: String },
    fieldId: { type: String },
  };

  message = '';
  saving = false;
  saveLabel = 'Save';
  cancelHref = '';
  fieldId = 'commit-message';

  private onInput(event: Event): void {
    const input = event.target as HTMLInputElement;
    this.message = input.value;
    this.dispatchEvent(
      new CustomEvent('fmn-message-change', { detail: { message: input.value }, bubbles: true }),
    );
  }

  override render(): TemplateResult {
    return html`
      <div class="commit-bar">
        <label for=${this.fieldId}>Commit message</label>
        <div class="commit-bar-row">
          <input
            id=${this.fieldId}
            name="message"
            type="text"
            maxlength="72"
            required
            .value=${this.message}
            @input=${this.onInput}
          />
          <button
            type="button"
            ?disabled=${this.saving || this.message.trim() === ''}
            aria-busy=${this.saving ? 'true' : 'false'}
            @click=${() => this.dispatchEvent(new CustomEvent('fmn-save', { bubbles: true }))}
          >
            ${this.saveLabel}
          </button>
          ${this.cancelHref === ''
            ? nothing
            : html`<a role="button" class="secondary outline" href=${this.cancelHref}>Cancel</a>`}
        </div>
      </div>
    `;
  }
}

customElements.define('fmn-commit-bar', FmnCommitBar);
