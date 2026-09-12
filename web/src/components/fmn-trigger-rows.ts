import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import type { Trigger, TriggerField } from '../api/types';
import { PageElement } from '../lib/element';
import { triggerFields } from '../model/triggers';

/** The editable trigger rows of a scope. */
export class FmnTriggerRows extends PageElement {
  static override properties: PropertyDeclarations = {
    triggers: { attribute: false },
  };

  triggers: Trigger[] = [];

  private emit(triggers: Trigger[]): void {
    this.triggers = triggers;
    this.dispatchEvent(new CustomEvent('fmn-triggers-change', { detail: { triggers }, bubbles: true }));
  }

  private replace(index: number, change: Partial<Trigger>): void {
    this.emit(this.triggers.map((trigger, at) => (at === index ? { ...trigger, ...change } : trigger)));
  }

  override render(): TemplateResult {
    return html`
      <div class="trigger-rows">
        ${this.triggers.map(
          (trigger, index) => html`<div class="trigger-row">
            <sl-select
              size="small"
              label="Field"
              value=${trigger.on}
              hoist
              @sl-change=${(event: Event) =>
                this.replace(index, { on: (event.target as HTMLInputElement).value as TriggerField })}
            >
              ${triggerFields.map((field) => html`<sl-option value=${field}>${field}</sl-option>`)}
            </sl-select>
            <sl-input
              size="small"
              label="Pattern"
              class="pattern"
              value=${trigger.pattern}
              @sl-input=${(event: Event) =>
                this.replace(index, { pattern: (event.target as HTMLInputElement).value })}
            ></sl-input>
            <sl-input
              size="small"
              label="Machine"
              value=${trigger.machine ?? ''}
              @sl-input=${(event: Event) =>
                this.replace(index, { machine: (event.target as HTMLInputElement).value })}
            ></sl-input>
            <sl-icon-button
              name="trash-2"
              label="Remove trigger"
              @click=${() => this.emit(this.triggers.filter((_, at) => at !== index))}
            ></sl-icon-button>
          </div>`,
        )}
      </div>
      <sl-button
        size="small"
        @click=${() => this.emit([...this.triggers, { on: 'user_message', pattern: '' }])}
      >
        <sl-icon slot="prefix" name="plus"></sl-icon>Add trigger
      </sl-button>
    `;
  }
}

customElements.define('fmn-trigger-rows', FmnTriggerRows);
