// The triggers of a scope, read and edited in the same table. Used wherever
// triggers are shown: a scope's page and a scope being created.

import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { Trigger, TriggerField } from '../api/types';
import { PageElement } from '../lib/element';
import { PatternChecker } from '../model/patternCheck';
import { fieldOf, takesMachine, triggerFieldChoices, withField, withMachine } from '../model/triggers';
import './fmn-editable-table';
import './fmn-machine';
import type { EditableColumn, RowEvent } from './fmn-editable-table';
import type { MachineChange } from './fmn-machine';

export class FmnTriggerTable extends PageElement {
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

  private get columns(): EditableColumn<Trigger>[] {
    return [
      {
        label: 'Field',
        width: 'minmax(0, 12rem)',
        cell: (trigger, index) => html`<sl-select
          size="small"
          class="cell-control"
          value=${fieldOf(trigger)}
          hoist
          @sl-change=${(event: Event) =>
            this.replace(index, withField(trigger, (event.target as HTMLInputElement).value as TriggerField))}
        >
          ${triggerFieldChoices.map((name) => html`<sl-option value=${name}>${name}</sl-option>`)}
        </sl-select>`,
      },
      {
        label: 'Pattern',
        width: 'minmax(0, 2fr)',
        cell: (trigger, index) => {
          const message = this.patterns.messageFor(trigger.pattern);
          return html`<sl-input
              size="small"
              class="cell-control pattern"
              value=${trigger.pattern}
              placeholder="A regular expression"
              ?data-invalid=${message !== null}
              @sl-input=${(event: Event) => {
                const pattern = (event.target as HTMLInputElement).value;
                this.patterns.schedule(pattern);
                this.replace(index, { ...trigger, pattern });
              }}
            ></sl-input>
            ${message === null ? nothing : html`<p class="field-error" role="alert">${message}</p>`}`;
        },
      },
      {
        label: 'Machine',
        width: 'minmax(0, 1fr)',
        cell: (trigger, index) =>
          takesMachine(fieldOf(trigger))
            ? html`<fmn-machine-field
                class="cell-control"
                machine=${trigger.machine ?? ''}
                .machines=${this.machines}
                @fmn-machine-change=${(event: CustomEvent<MachineChange>) => {
                  event.stopPropagation();
                  this.replace(index, withMachine(trigger, event.detail.machine));
                }}
              ></fmn-machine-field>`
            : html`<sl-tooltip content="Only an any or working_directory trigger names a machine">
                <fmn-machine-value machine=""></fmn-machine-value>
              </sl-tooltip>`,
      },
    ];
  }

  override render(): TemplateResult {
    return html`<fmn-editable-table
      class="trigger-table"
      .columns=${this.columns}
      .rows=${this.triggers}
      addLabel="Add trigger"
      removeLabel="Remove trigger"
      emptyText="No triggers"
      @fmn-row-add=${() => this.emit([...this.triggers, { pattern: '' }])}
      @fmn-row-remove=${(event: CustomEvent<RowEvent>) =>
        this.emit(this.triggers.filter((_, at) => at !== event.detail.index))}
    ></fmn-editable-table>`;
  }
}

customElements.define('fmn-trigger-table', FmnTriggerTable);
