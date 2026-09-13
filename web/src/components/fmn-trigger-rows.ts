import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { Trigger, TriggerField } from '../api/types';
import { PageElement } from '../lib/element';
import { PatternChecker } from '../model/patternCheck';
import { fieldOf, takesMachine, triggerFieldChoices, withField, withMachine } from '../model/triggers';
import './fmn-tag-field';
import type { TagsChange } from './fmn-tag-field';

/** The editable trigger rows of a scope. */
export class FmnTriggerRows extends PageElement {
  static override properties: PropertyDeclarations = {
    triggers: { attribute: false },
    machines: { attribute: false },
  };

  triggers: Trigger[] = [];
  /** The machine names offered under the machine field. */
  machines: string[] = [];

  /** Asks the server about a pattern once the typing stops, and keeps the answer. */
  private readonly patterns = new PatternChecker(
    (pattern) => api.validatePattern(pattern),
    250,
    () => this.requestUpdate(),
  );

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.patterns.cancel();
  }

  private emit(triggers: Trigger[]): void {
    this.triggers = triggers;
    this.dispatchEvent(new CustomEvent('fmn-triggers-change', { detail: { triggers }, bubbles: true }));
  }

  private replace(index: number, trigger: Trigger): void {
    this.emit(this.triggers.map((existing, at) => (at === index ? trigger : existing)));
  }

  private renderRow(trigger: Trigger, index: number): TemplateResult {
    const field = fieldOf(trigger);
    const message = this.patterns.messageFor(trigger.pattern);
    return html`<div class="trigger-row">
      <sl-select
        size="small"
        label="Field"
        value=${field}
        hoist
        @sl-change=${(event: Event) =>
          this.replace(index, withField(trigger, (event.target as HTMLInputElement).value as TriggerField))}
      >
        ${triggerFieldChoices.map((name) => html`<sl-option value=${name}>${name}</sl-option>`)}
      </sl-select>
      <div class="pattern-field">
        <sl-input
          size="small"
          label="Pattern"
          class="pattern"
          value=${trigger.pattern}
          ?data-invalid=${message !== null}
          @sl-input=${(event: Event) => {
            const pattern = (event.target as HTMLInputElement).value;
            this.patterns.schedule(pattern);
            this.replace(index, { ...trigger, pattern });
          }}
        ></sl-input>
        ${message === null
          ? nothing
          : html`<p class="field-error" role="alert">${message}</p>`}
      </div>
      ${takesMachine(field)
        ? html`<fmn-tag-field
            single
            showLabel
            label="Machine"
            placeholder="Any machine"
            .value=${trigger.machine === undefined ? [] : [trigger.machine]}
            .suggestions=${this.machines}
            @fmn-tags-change=${(event: CustomEvent<TagsChange>) =>
              this.replace(index, withMachine(trigger, event.detail.value[0] ?? ''))}
          ></fmn-tag-field>`
        : html`<sl-tooltip content="Only an any or working_directory trigger names a machine">
            <sl-input size="small" label="Machine" placeholder="—" disabled></sl-input>
          </sl-tooltip>`}
      <sl-icon-button
        name="trash"
        label="Remove trigger"
        @click=${() => this.emit(this.triggers.filter((_, at) => at !== index))}
      ></sl-icon-button>
    </div>`;
  }

  override render(): TemplateResult {
    return html`
      <div class="trigger-rows">${this.triggers.map((trigger, index) => this.renderRow(trigger, index))}</div>
      <sl-button size="small" @click=${() => this.emit([...this.triggers, { pattern: '' }])}>
        <sl-icon slot="prefix" name="plus"></sl-icon>Add trigger
      </sl-button>
    `;
  }
}

customElements.define('fmn-trigger-rows', FmnTriggerRows);
