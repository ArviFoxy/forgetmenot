import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

/** The one bar that appears once something has changed: message, save, discard. */
export class FmnCommitBar extends PageElement {
  static override properties: PropertyDeclarations = {
    message: { type: String },
    saving: { type: Boolean },
    saveLabel: { type: String },
    label: { type: String },
  };

  message = '';
  saving = false;
  saveLabel = 'Save';
  label = 'Unsaved changes';

  override render(): TemplateResult {
    return html`
      <div class="commit-bar" role="group" aria-label=${this.label}>
        <span class="commit-label">${this.label}</span>
        <sl-input
          size="small"
          label="Commit message"
          placeholder="What changed"
          maxlength="72"
          value=${this.message}
          @sl-input=${(event: Event) => {
            const value = (event.target as HTMLInputElement).value;
            this.message = value;
            this.dispatchEvent(
              new CustomEvent('fmn-message-change', { detail: { message: value }, bubbles: true }),
            );
          }}
        ></sl-input>
        <sl-button
          size="small"
          variant="primary"
          ?disabled=${this.saving || this.message.trim() === ''}
          ?loading=${this.saving}
          @click=${() => this.dispatchEvent(new CustomEvent('fmn-save', { bubbles: true }))}
          >${this.saveLabel}</sl-button
        >
        <sl-button
          size="small"
          variant="default"
          ?disabled=${this.saving}
          @click=${() => this.dispatchEvent(new CustomEvent('fmn-discard', { bubbles: true }))}
        >
          <sl-icon slot="prefix" name="rotate-ccw"></sl-icon>Discard
        </sl-button>
      </div>
    `;
  }
}

customElements.define('fmn-commit-bar', FmnCommitBar);
