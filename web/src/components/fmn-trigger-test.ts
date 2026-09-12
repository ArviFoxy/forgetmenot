import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { TriggerField, TriggerTestResult } from '../api/types';
import { PageElement } from '../lib/element';
import { paths } from '../routes';
import { triggerFields } from '../model/triggers';

/** Sends a field, a text and a machine to the trigger test endpoint and lists what fired. */
export class FmnTriggerTest extends PageElement {
  static override properties: PropertyDeclarations = {
    heading: { type: String },
    idPrefix: { type: String },
    field: { state: true },
    text: { state: true },
    machine: { state: true },
    result: { state: true },
    failure: { state: true },
  };

  heading = 'Trigger test';
  idPrefix = 'test';

  private field: TriggerField = 'user_message';
  private text = '';
  private machine = '';
  private result: TriggerTestResult | null = null;
  private failure: string | null = null;

  private async run(): Promise<void> {
    this.failure = null;
    try {
      this.result = await api.testTriggers({ field: this.field, text: this.text, machine: this.machine });
    } catch (caught) {
      this.result = null;
      this.failure = caught instanceof Error ? caught.message : String(caught);
    }
  }

  override render(): TemplateResult {
    const fieldId = `${this.idPrefix}-field`;
    const machineId = `${this.idPrefix}-machine`;
    const textId = `${this.idPrefix}-text`;
    return html`
      <section class="trigger-test">
        <h2>${this.heading}</h2>
        <div class="field-grid">
          <label for=${fieldId}>Field</label>
          <select
            id=${fieldId}
            @change=${(event: Event) => {
              this.field = (event.target as HTMLSelectElement).value as TriggerField;
            }}
          >
            ${triggerFields.map(
              (name) => html`<option value=${name} ?selected=${name === this.field}>${name}</option>`,
            )}
          </select>

          <label for=${machineId}>Machine</label>
          <input
            id=${machineId}
            type="text"
            .value=${this.machine}
            @input=${(event: Event) => {
              this.machine = (event.target as HTMLInputElement).value;
            }}
          />

          <label for=${textId}>Text</label>
          <textarea
            id=${textId}
            rows="4"
            .value=${this.text}
            @input=${(event: Event) => {
              this.text = (event.target as HTMLTextAreaElement).value;
            }}
          ></textarea>
        </div>
        <button type="button" @click=${() => void this.run()}>Test</button>

        ${this.failure === null
          ? nothing
          : html`<p class="failure" role="alert">${this.failure}</p>`}
        ${this.result === null
          ? nothing
          : html`<figure>
              <table>
                <thead>
                  <tr>
                    <th scope="col">Scope</th>
                    <th scope="col">Field</th>
                    <th scope="col">Pattern</th>
                  </tr>
                </thead>
                <tbody>
                  ${this.result.fired.map(
                    (match) => html`<tr>
                      <td><a href=${paths.scope(match.scope_id)}>${match.scope_id}</a></td>
                      <td>${match.field}</td>
                      <td><code>${match.pattern}</code></td>
                    </tr>`,
                  )}
                </tbody>
              </table>
            </figure>`}
      </section>
    `;
  }
}

customElements.define('fmn-trigger-test', FmnTriggerTest);
