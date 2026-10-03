import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

/** What the bar holds: the commit's title line and the rest of its message. */
export interface CommitText {
  commitTitle: string;
  commitBody: string;
}

/**
 * The one bar that appears once something has changed: commit title, commit
 * description, save, discard. Only the title is required.
 */
export class FmnCommitBar extends PageElement {
  static override properties: PropertyDeclarations = {
    commitTitle: { type: String },
    commitBody: { type: String },
    saving: { type: Boolean },
    saveLabel: { type: String },
    label: { type: String },
  };

  commitTitle = '';
  commitBody = '';
  saving = false;
  saveLabel = 'Save';
  label = 'Unsaved changes';

  private announce(): void {
    const detail: CommitText = { commitTitle: this.commitTitle, commitBody: this.commitBody };
    this.dispatchEvent(new CustomEvent<CommitText>('fmn-commit-change', { detail, bubbles: true }));
  }

  override render(): TemplateResult {
    return html`
      <div class="commit-bar" role="group" aria-label=${this.label}>
        <sl-input
          class="commit-title"
          size="small"
          label="Commit title"
          placeholder="What changed"
          maxlength="72"
          value=${this.commitTitle}
          @sl-input=${(event: Event) => {
            this.commitTitle = (event.target as HTMLInputElement).value;
            this.announce();
          }}
        ></sl-input>
        <sl-textarea
          class="commit-description"
          size="small"
          label="Commit description"
          rows="2"
          resize="auto"
          value=${this.commitBody}
          @sl-input=${(event: Event) => {
            this.commitBody = (event.target as HTMLTextAreaElement).value;
            this.announce();
          }}
        ></sl-textarea>
        <sl-button
          class="commit-save"
          size="small"
          variant="primary"
          ?disabled=${this.saving || this.commitTitle.trim() === ''}
          ?loading=${this.saving}
          @click=${() => this.dispatchEvent(new CustomEvent('fmn-save', { bubbles: true }))}
          >${this.saveLabel}</sl-button
        >
        <sl-button
          class="commit-discard"
          size="small"
          variant="default"
          ?disabled=${this.saving}
          @click=${() => this.dispatchEvent(new CustomEvent('fmn-discard', { bubbles: true }))}
        >
          <sl-icon slot="prefix" name="rotate"></sl-icon>Discard
        </sl-button>
      </div>
    `;
  }
}

customElements.define('fmn-commit-bar', FmnCommitBar);
