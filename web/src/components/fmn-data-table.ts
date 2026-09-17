// One sortable, filterable table. TanStack Table keeps the sorting and the filter
// and decides which rows are shown; the markup is this file's, so the result is the
// same `table.data` every other page draws.

import { html, nothing, type PropertyDeclarations, type TemplateResult } from 'lit';
import {
  TableController,
  columnFilteringFeature,
  createFilteredRowModel,
  createSortedRowModel,
  filterFn_includesString,
  globalFilteringFeature,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  type ColumnDef,
} from '@tanstack/lit-table';
import { PageElement } from '../lib/element';
import { cellText, type TableColumn } from '../model/stats';

/** A row of any shape, since the columns say what to read out of it. */
export type DataRow = Record<string, unknown>;

const features = tableFeatures({
  columnFilteringFeature,
  globalFilteringFeature,
  filteredRowModel: createFilteredRowModel(),
  filterFns: { includesString: filterFn_includesString },
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, basic: sortFn_basic },
});

/** What a click on a row reports: the row itself. */
export interface RowClick<Row> {
  row: Row;
}

/**
 * The rows of one report. `columns` says what each column holds and where it
 * leads, `expand` what a row opens below itself, and a click anywhere else on a
 * row is reported as `fmn-row-click`.
 */
export class FmnDataTable<Row extends DataRow = DataRow> extends PageElement {
  static override properties: PropertyDeclarations = {
    columns: { attribute: false },
    rows: { attribute: false },
    rowKey: { attribute: false },
    expand: { attribute: false },
    empty: { type: String },
    filterLabel: { type: String },
    filterText: { state: true },
    open: { state: true },
  };

  columns: TableColumn<Row>[] = [];
  rows: Row[] = [];
  /** What tells one row from another, for the set of open rows. */
  rowKey: (row: Row) => string = (row) => JSON.stringify(row);
  /** What an open row shows below itself; null when rows do not open. */
  expand: ((row: Row) => TemplateResult) | null = null;
  empty = 'Nothing in this range';
  filterLabel = 'Filter';

  private filterText = '';
  private open = new Set<string>();

  private readonly controller = new TableController<typeof features, Row>(this);

  /** The column definitions TanStack reads, rebuilt when the columns change. */
  private definitions: ColumnDef<typeof features, Row>[] = [];
  private definedFor: TableColumn<Row>[] | null = null;

  private tanstackColumns(): ColumnDef<typeof features, Row>[] {
    if (this.definedFor === this.columns) return this.definitions;
    this.definedFor = this.columns;
    this.definitions = this.columns.map((column) => ({
      id: column.id,
      header: column.header,
      // The column's own reading of the row, so sorting a formatted figure sorts
      // by the number and filtering matches the text on screen.
      accessorFn: (row: Row) => column.value(row),
      sortingFn: column.numeric === true ? ('basic' as const) : ('alphanumeric' as const),
      filterFn: 'includesString' as const,
    }));
    return this.definitions;
  }

  private toggle(key: string): void {
    const open = new Set(this.open);
    if (open.has(key)) open.delete(key);
    else open.add(key);
    this.open = open;
  }

  /**
   * A click on the row itself. A click on a link or on the chevron is that
   * control's own, so the row does not act on it as well.
   */
  private clicked(event: MouseEvent, row: Row): void {
    const target = event.target;
    if (target instanceof Element && target.closest('a, sl-icon-button') !== null) return;
    this.dispatchEvent(
      new CustomEvent<RowClick<Row>>('fmn-row-click', { detail: { row }, bubbles: true }),
    );
  }

  private renderCell(column: TableColumn<Row>, row: Row): TemplateResult {
    const shown = cellText(column, row);
    const text = column.mono === true ? html`<code>${shown}</code>` : shown;
    const target = column.link?.(row) ?? null;
    const classes = [
      column.numeric === true ? 'number' : '',
      column.moment === true ? 'moment nowrap' : '',
    ]
      .join(' ')
      .trim();
    return html`<td class=${classes}>
      ${target === null ? text : html`<a href=${target}>${text}</a>`}
    </td>`;
  }

  override render(): TemplateResult {
    const table = this.controller.table({
      features,
      columns: this.tanstackColumns(),
      data: this.rows,
      // The field above the table holds the filter text, so the table is told
      // what it is rather than keeping its own; nothing here calls the setter
      // the option pairs with.
      state: { globalFilter: this.filterText },
      globalFilterFn: 'includesString' as const,
      onGlobalFilterChange: () => undefined,
    });
    const shown = table.getRowModel().rows;
    const width = this.columns.length + (this.expand === null ? 0 : 1);
    return html`
      <div class="table-filter">
        <sl-input
          size="small"
          clearable
          placeholder=${this.filterLabel}
          value=${this.filterText}
          @sl-input=${(event: Event) => {
            this.filterText = (event.target as HTMLInputElement).value;
          }}
        ></sl-input>
      </div>
      <div class="table-wrap">
        <table class="data stats">
          <thead>
            <tr>
              ${this.expand === null ? nothing : html`<th scope="col" class="expander"></th>`}
              ${table.getHeaderGroups()[0]?.headers.map((header) => {
                const sorted = header.column.getIsSorted();
                return html`<th
                  scope="col"
                  class=${this.columns.find((column) => column.id === header.column.id)?.numeric ===
                  true
                    ? 'number'
                    : ''}
                  aria-sort=${sorted === 'asc'
                    ? 'ascending'
                    : sorted === 'desc'
                      ? 'descending'
                      : 'none'}
                >
                  <button
                    type="button"
                    class="sort"
                    @click=${() => header.column.toggleSorting()}
                  >
                    ${String(header.column.columnDef.header ?? header.column.id)}
                    <sl-icon
                      name=${sorted === 'asc'
                        ? 'chevron-up'
                        : sorted === 'desc'
                          ? 'chevron-down'
                          : 'selector'}
                    ></sl-icon>
                  </button>
                </th>`;
              })}
            </tr>
          </thead>
          <tbody>
            ${shown.length === 0
              ? html`<tr class="empty-row">
                  <td colspan=${width}><span class="empty">${this.empty}</span></td>
                </tr>`
              : shown.map((modelRow) => {
                  const row = modelRow.original;
                  const key = this.rowKey(row);
                  const isOpen = this.open.has(key);
                  return html`<tr
                      class="data-row"
                      @click=${(event: MouseEvent) => this.clicked(event, row)}
                    >
                      ${this.expand === null
                        ? nothing
                        : html`<td class="expander">
                            <sl-icon-button
                              name=${isOpen ? 'chevron-down' : 'chevron-right'}
                              label=${isOpen ? 'Collapse' : 'Expand'}
                              @click=${() => this.toggle(key)}
                            ></sl-icon-button>
                          </td>`}
                      ${this.columns.map((column) => this.renderCell(column, row))}
                    </tr>
                    ${isOpen && this.expand !== null
                      ? html`<tr class="sub-row">
                          <td colspan=${width}>${this.expand(row)}</td>
                        </tr>`
                      : nothing}`;
                })}
          </tbody>
        </table>
      </div>
    `;
  }
}

customElements.define('fmn-data-table', FmnDataTable);
