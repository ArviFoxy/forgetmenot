// Everywhere a machine name appears: the value as a reader sees it, and the field
// that changes it. One module, so the word a machine is shown by and the list it is
// picked from cannot differ between the trigger table, the trigger test and
// whatever names a machine next.

import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import { machineDisplay } from '../model/triggers';
import './fmn-tag-field';
import type { TagsChange } from './fmn-tag-field';

/** The machine a thing is limited to, or `Any` when it is limited to none. */
export class FmnMachineValue extends PageElement {
  static override properties: PropertyDeclarations = {
    machine: { type: String },
  };

  machine = '';

  override render(): TemplateResult {
    const shown = machineDisplay(this.machine);
    // `Any` is set apart from a name, because a machine could be called that.
    return shown.any
      ? html`<em class="machine-any muted">${shown.text}</em>`
      : html`<span class="machine-name">${shown.text}</span>`;
  }
}

/** One machine name, typed freely or picked from the ones the server knows. */
export class FmnMachineField extends PageElement {
  static override properties: PropertyDeclarations = {
    machine: { type: String },
    machines: { attribute: false },
    label: { type: String },
    showLabel: { type: Boolean },
  };

  machine = '';
  machines: string[] = [];
  label = 'Machine';
  showLabel = false;

  override render(): TemplateResult {
    return html`<fmn-tag-field
      single
      label=${this.label}
      ?showLabel=${this.showLabel}
      placeholder="Any machine"
      .value=${this.machine === '' ? [] : [this.machine]}
      .suggestions=${this.machines}
      @fmn-tags-change=${(event: CustomEvent<TagsChange>) => {
        event.stopPropagation();
        const machine = event.detail.value[0] ?? '';
        this.machine = machine;
        this.dispatchEvent(
          new CustomEvent('fmn-machine-change', { detail: { machine }, bubbles: true }),
        );
      }}
    ></fmn-tag-field>`;
  }
}

export interface MachineChange {
  machine: string;
}

customElements.define('fmn-machine-value', FmnMachineValue);
customElements.define('fmn-machine-field', FmnMachineField);
