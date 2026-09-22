// One sortable, filterable table, whose rows may hold rows of their own. TanStack
// Table keeps the sorting, the filter and the nesting and decides which rows are
// shown; the markup is this file's, so the result is the same `table.data` every
// other page draws.

import { html, nothing, type PropertyDeclarations, type TemplateResult } from 'lit';
import {
  TableController,
  columnFilteringFeature,
  createExpandedRowModel,
  createFilteredRowModel,
  createSortedRowModel,
  filterFn_includesString,
  globalFilteringFeature,
  rowExpandingFeature,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  type ColumnDef,
} from '@tanstack/lit-table';
import { PageElement } from '../lib/element';
import {
  cellText,
  columnBreakpoints,
  hiddenColumns,
  visibleColumns,
  type TableColumn,
  type TableSort,
} from '../model/stats';

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
  rowExpandingFeature,
  expandedRowModel: createExpandedRowModel(),
});

/** What a click on a row reports: the row itself. */
export interface RowClick<Row> {
  row: Row;
}

/** A row of the table's own model, which holds the rows nested under it. */
interface NestedRow {
  subRows: readonly NestedRow[];
}

/** The rows below one row at any depth, which is what its closed form stands for. */
function descendantCount(row: NestedRow): number {
  return row.subRows.reduce((below, child) => below + 1 + descendantCount(child), 0);
}

/**
 * The rows of one report. `columns` says what each column holds and where it
 * leads, `subRows` what hangs under a row, `expand` what a row opens below
 * itself, and a click anywhere else on a row is reported as `fmn-row-click`.
 *
 * A column is drawn while the viewport is wide enough for its priority; the
 * narrower the screen, the fewer columns, and an open row lists what the width
 * left out, so every figure is reachable at every size.
 *
 * Every row starts closed, so a row with rows under it reads as one row and the
 * number it stands for until it is opened.
 */
export class FmnDataTable<Row extends DataRow = DataRow> extends PageElement {
  static override properties: PropertyDeclarations = {
    columns: { attribute: false },
    rows: { attribute: false },
    rowKey: { attribute: false },
    subRows: { attribute: false },
    expand: { attribute: false },
    sort: { attribute: false },
    empty: { type: String },
    filterLabel: { type: String },
    filterText: { state: true },
    open: { state: true },
    width: { state: true },
  };

  columns: TableColumn<Row>[] = [];
  rows: Row[] = [];
  /** What tells one row from another, for the set of open rows. */
  rowKey: (row: Row) => string = (row) => JSON.stringify(row);
  /** The rows nested under one row, at any depth; null when rows do not nest. */
  subRows: ((row: Row) => Row[]) | null = null;
  /** What an open row shows below itself; null when rows do not open. */
  expand: ((row: Row) => TemplateResult) | null = null;
  /** What the table is sorted by until a header is clicked; unsorted when null. */
  sort: TableSort | null = null;
  empty = 'Nothing in this range';
  filterLabel = 'Filter';

  private filterText = '';
  private open = new Set<string>();
  /**
   * The same keys as `open`, in the shape TanStack reads them. It is kept rather
   * than built in `render`, because the table compares the state it is given
   * against the state it holds by identity and a fresh object every render is a
   * change every render.
   */
  private openRows: Record<string, boolean> = {};
  /** The viewport width the columns are chosen against. */
  private width = window.innerWidth;
  private stopListening: (() => void)[] = [];

  override connectedCallback(): void {
    super.connectedCallback();
    this.width = window.innerWidth;
    // One listener per width at which the columns change, so the table is
    // redrawn when a rotation or a resize crosses one of them.
    this.stopListening = columnBreakpoints.map((breakpoint) => {
      const media = window.matchMedia(`(min-width: ${breakpoint}px)`);
      const crossed = (): void => {
        this.width = window.innerWidth;
      };
      media.addEventListener('change', crossed);
      return () => media.removeEventListener('change', crossed);
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    for (const stop of this.stopListening) stop();
    this.stopListening = [];
  }

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
      // A figure is read largest first, so the first click on one sorts down and
      // the next click flips it.
      sortDescFirst: column.numeric === true,
      filterFn: 'includesString' as const,
    }));
    return this.definitions;
  }

  private toggle(key: string): void {
    const open = new Set(this.open);
    if (open.has(key)) open.delete(key);
    else open.add(key);
    this.open = open;
    this.openRows = Object.fromEntries([...open].map((each) => [each, true]));
  }

  /**
   * A click on the row itself. A click on a link, on the chevron or on the count
   * beside it is that control's own, so the row does not act on it as well.
   */
  private clicked(event: MouseEvent, row: Row): void {
    const target = event.target;
    if (target instanceof Element && target.closest('a, sl-icon-button, .descendants') !== null) {
      return;
    }
    this.dispatchEvent(
      new CustomEvent<RowClick<Row>>('fmn-row-click', { detail: { row }, bubbles: true }),
    );
  }

  /** What one column holds for one row: its own markup, or the text it reads as. */
  private renderValue(column: TableColumn<Row>, row: Row): TemplateResult | string {
    if (column.cell !== undefined) return column.cell(row);
    const shown = cellText(column, row);
    return column.mono === true ? html`<code>${shown}</code>` : shown;
  }

  /**
   * What the width left out of the row, so no figure is out of reach on a narrow
   * screen; nothing when every column is drawn.
   */
  private renderHiddenCells(hidden: TableColumn<Row>[], row: Row): TemplateResult | typeof nothing {
    if (hidden.length === 0) return nothing;
    return html`<dl class="hidden-cells">
      ${hidden.map(
        (column) => html`<dt>${column.header}</dt>
          <dd>${this.renderValue(column, row)}</dd>`,
      )}
    </dl>`;
  }

  /**
   * One cell. `first` marks the cell that names the row, which stays in place
   * while a narrow screen scrolls the rest of the row past it and which carries
   * the indent of the row's depth.
   */
  private renderCell(
    column: TableColumn<Row>,
    row: Row,
    first: boolean,
    depth: number,
  ): TemplateResult {
    const content = this.renderValue(column, row);
    const target = column.link?.(row) ?? null;
    const classes = [
      column.numeric === true ? 'number' : '',
      column.moment === true ? 'moment nowrap' : '',
      column.wraps === true ? 'wrap' : '',
      first ? 'row-id' : '',
    ]
      .join(' ')
      .trim();
    return html`<td
      class=${classes}
      style=${first && depth > 0
        ? `padding-inline-start: calc(var(--sl-spacing-x-small) + ${depth} * var(--fmn-nesting-step))`
        : nothing}
    >
      ${target === null ? content : html`<a href=${target}>${content}</a>`}
    </td>`;
  }

  override render(): TemplateResult {
    const table = this.controller.table({
      features,
      columns: this.tanstackColumns(),
      data: this.rows,
      // A row is told apart by its key everywhere, so the rows this table holds
      // open and the rows TanStack draws the children of are the same keys.
      getRowId: (row: Row) => this.rowKey(row),
      getSubRows: this.subRows === null ? undefined : (row: Row) => this.subRows?.(row),
      // A row is kept when it matches or anything under it does, so a filter
      // naming a nested row finds it under the rows it hangs from rather than
      // dropping it with the parent that does not match.
      filterFromLeafRows: true,
      // The field above the table holds the filter text and this element holds
      // the open rows, so the table is told what both are rather than keeping
      // its own; nothing here calls the setters the options pair with. While
      // the field has text in it every row is open, so a row the filter kept is
      // on screen rather than behind a row that happens to be closed.
      state: {
        globalFilter: this.filterText,
        expanded: this.filterText === '' ? this.openRows : true,
      },
      globalFilterFn: 'includesString' as const,
      onGlobalFilterChange: () => undefined,
      onExpandedChange: () => undefined,
      // Read once, when the table is built: what it opens on, before anyone has
      // clicked a header.
      initialState: { sorting: this.sort === null ? [] : [this.sort] },
    });
    const shown = table.getRowModel().rows;
    const columns = visibleColumns(this.columns, this.width);
    const hidden = hiddenColumns(this.columns, this.width);
    // A row opens for what it holds as well as for what it leads to, so a table
    // whose columns do not all fit still opens.
    const opensDetail = this.expand !== null || hidden.length > 0;
    const nests = shown.some((modelRow) => modelRow.subRows.length > 0);
    const opens = opensDetail || nests;
    const width = columns.length + (opens ? 1 : 0);
    const headers = (table.getHeaderGroups()[0]?.headers ?? []).filter((header) =>
      columns.some((column) => column.id === header.column.id),
    );
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
              ${opens ? html`<th scope="col" class="expander"></th>` : nothing}
              ${headers.map((header, index) => {
                const sorted = header.column.getIsSorted();
                const numeric =
                  columns.find((column) => column.id === header.column.id)?.numeric === true;
                return html`<th
                  scope="col"
                  class=${[numeric ? 'number' : '', index === 0 ? 'row-id' : '']
                    .join(' ')
                    .trim()}
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
                  const below = descendantCount(modelRow);
                  return html`<tr
                      class="data-row"
                      @click=${(event: MouseEvent) => this.clicked(event, row)}
                    >
                      ${opens
                        ? html`<td class="expander">
                            ${opensDetail || below > 0
                              ? html`<sl-icon-button
                                    name=${isOpen ? 'chevron-down' : 'chevron-right'}
                                    label=${isOpen ? 'Collapse' : 'Expand'}
                                    @click=${() => this.toggle(key)}
                                  ></sl-icon-button>
                                  ${below === 0
                                    ? nothing
                                    : html`<span class="descendants">${below}</span>`}`
                              : nothing}
                          </td>`
                        : nothing}
                      ${columns.map((column, index) =>
                        this.renderCell(column, row, index === 0, modelRow.depth),
                      )}
                    </tr>
                    ${isOpen && opensDetail
                      ? html`<tr class="sub-row">
                          <td colspan=${width}>
                            ${this.renderHiddenCells(hidden, row)}${this.expand?.(row) ?? nothing}
                          </td>
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
