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
    return html`
      <section class="trigger-test">
        <h2>${this.heading}</h2>
        <div class="field-grid">
          <sl-select
            size="small"
            label="Field"
            value=${this.field}
            hoist
            @sl-change=${(event: Event) => {
              this.field = (event.target as HTMLInputElement).value as TriggerField;
            }}
          >
            ${triggerFields.map((name) => html`<sl-option value=${name}>${name}</sl-option>`)}
          </sl-select>
          <sl-input
            size="small"
            label="Machine"
            value=${this.machine}
            @sl-input=${(event: Event) => {
              this.machine = (event.target as HTMLInputElement).value;
            }}
          ></sl-input>
          <sl-textarea
            size="small"
            label="Text"
            rows="3"
            value=${this.text}
            @sl-input=${(event: Event) => {
              this.text = (event.target as HTMLInputElement).value;
            }}
          ></sl-textarea>
          <div class="actions">
            <sl-button size="small" variant="primary" @click=${() => void this.run()}>Test</sl-button>
          </div>
        </div>

        ${this.failure === null ? nothing : html`<p class="failure" role="alert">${this.failure}</p>`}
        ${this.result === null
          ? nothing
          : this.result.fired.length === 0
            ? html`<p class="empty">Nothing fired</p>`
            : html`<div class="table-wrap">
                <table class="data">
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
                        <td class="nowrap"><a href=${paths.scope(match.scope_id)}>${match.scope_id}</a></td>
                        <td class="nowrap">${match.field}</td>
                        <td><code>${match.pattern}</code></td>
                      </tr>`,
                    )}
                  </tbody>
                </table>
              </div>`}
      </section>
    `;
  }
}

customElements.define('fmn-trigger-test', FmnTriggerTest);
