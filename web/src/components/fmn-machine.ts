// The field a machine name is typed into or picked from, shared by everything that
// names a machine, so the list it is picked from cannot differ between places.

import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import './fmn-tag-field';
import type { TagsChange } from './fmn-tag-field';

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

customElements.define('fmn-machine-field', FmnMachineField);
