// A table that is read and written in the same place. Left alone it reads like a
// table in a document: no boxes, no buttons shouting to be pressed. Under the
// pointer or the caret the cell it is on shows its control, so the row can be
// changed where it is read rather than in a form somewhere else.
//
// The component owns the frame: the head, the rows, the control that adds one and
// the one that removes one. What a cell holds is the caller's, given as a column
// with a renderer, so a second table of something else reuses all of this.

import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';

export interface EditableColumn<Row> {
  /** Named for the reader; also what the cell is labelled by for a screen reader. */
  label: string;
  /** The control, which is what is shown whether or not the row is being edited. */
  cell: (row: Row, index: number) => TemplateResult;
  /** A CSS grid track, so a pattern may take more room than a field name. */
  width?: string;
}

export interface RowEvent {
  index: number;
}

/**
 * `rows` and `columns` are set as properties. The table asks for a row to be added
 * or removed and never changes the list itself: the page that owns the data does
 * that, so one list stays the truth.
 */
export class FmnEditableTable<Row = unknown> extends PageElement {
  static override properties: PropertyDeclarations = {
    columns: { attribute: false },
    rows: { attribute: false },
    addLabel: { type: String },
    removeLabel: { type: String },
    emptyText: { type: String },
  };

  columns: EditableColumn<Row>[] = [];
  rows: Row[] = [];
  addLabel = 'Add row';
  removeLabel = 'Remove row';
  emptyText = 'Nothing yet';

  private emit(name: 'fmn-row-add' | 'fmn-row-remove', index: number): void {
    this.dispatchEvent(new CustomEvent<RowEvent>(name, { detail: { index }, bubbles: true }));
  }

  private get template(): string {
    return [...this.columns.map((column) => column.width ?? 'minmax(0, 1fr)'), 'auto'].join(' ');
  }

  override render(): TemplateResult {
    return html`
      <div class="editable-table" role="table" style="--fmn-table-columns: ${this.template}">
        <div class="editable-row editable-head" role="row">
          ${this.columns.map(
            (column) => html`<span class="editable-cell" role="columnheader">${column.label}</span>`,
          )}
          <span class="editable-cell editable-actions" role="columnheader"></span>
        </div>
        ${this.rows.length === 0
          ? html`<p class="empty">${this.emptyText}</p>`
          : this.rows.map(
              (row, index) => html`<div class="editable-row" role="row">
                ${this.columns.map(
                  (column) => html`<span class="editable-cell" role="cell"
                    >${column.cell(row, index)}</span
                  >`,
                )}
                <span class="editable-cell editable-actions" role="cell">
                  <sl-icon-button
                    name="trash"
                    label=${`${this.removeLabel} ${index + 1}`}
                    @click=${() => this.emit('fmn-row-remove', index)}
                  ></sl-icon-button>
                </span>
              </div>`,
            )}
        <div class="editable-row editable-add" role="row">
          <sl-button size="small" variant="text" @click=${() => this.emit('fmn-row-add', this.rows.length)}>
            <sl-icon slot="prefix" name="plus"></sl-icon>${this.addLabel}
          </sl-button>
        </div>
      </div>
      ${nothing}
    `;
  }
}

customElements.define('fmn-editable-table', FmnEditableTable);
