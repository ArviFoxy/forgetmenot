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
    this.emit(
      this.triggers.map((trigger, at) => (at === index ? { ...trigger, ...change } : trigger)),
    );
  }

  override render(): TemplateResult {
    return html`
      <figure>
        <table>
          <thead>
            <tr>
              <th scope="col">Field</th>
              <th scope="col">Pattern</th>
              <th scope="col">Machine</th>
              <th scope="col"><span class="sr-only">Remove</span></th>
            </tr>
          </thead>
          <tbody>
            ${this.triggers.map(
              (trigger, index) => html`<tr>
                <td>
                  <select
                    aria-label="Trigger field"
                    .value=${trigger.on}
                    @change=${(event: Event) =>
                      this.replace(index, { on: (event.target as HTMLSelectElement).value as TriggerField })}
                  >
                    ${triggerFields.map(
                      (field) =>
                        html`<option value=${field} ?selected=${field === trigger.on}>${field}</option>`,
                    )}
                  </select>
                </td>
                <td>
                  <input
                    type="text"
                    aria-label="Trigger pattern"
                    .value=${trigger.pattern}
                    @input=${(event: Event) =>
                      this.replace(index, { pattern: (event.target as HTMLInputElement).value })}
                  />
                </td>
                <td>
                  <input
                    type="text"
                    aria-label="Trigger machine"
                    .value=${trigger.machine ?? ''}
                    @input=${(event: Event) =>
                      this.replace(index, { machine: (event.target as HTMLInputElement).value })}
                  />
                </td>
                <td>
                  <button
                    type="button"
                    class="secondary outline"
                    @click=${() => this.emit(this.triggers.filter((_, at) => at !== index))}
                  >
                    Remove
                  </button>
                </td>
              </tr>`,
            )}
          </tbody>
        </table>
      </figure>
      <button
        type="button"
        class="outline"
        @click=${() => this.emit([...this.triggers, { on: 'user_message', pattern: '' }])}
      >
        Add trigger
      </button>
    `;
  }
}

customElements.define('fmn-trigger-rows', FmnTriggerRows);
