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
 * narrower the screen, the fewer columns.
 *
 * A table whose rows do not nest opens a row into a panel below it, which lists
 * what the width left out and what `expand` draws. A table given `subRows` is a
 * tree: its chevron sits in the cell that names the row and opens the rows under
 * it and nothing else, and it has no panel, so its columns say themselves what a
 * narrow screen keeps of a row. Every row starts closed, so a row with rows
 * under it reads as one row and the number it stands for until it is opened.
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
    detailOpen: { state: true },
    childrenOpen: { state: true },
    width: { state: true },
  };

  columns: TableColumn<Row>[] = [];
  rows: Row[] = [];
  /** What tells one row from another, for the set of open rows. */
  rowKey: (row: Row) => string = (row) => JSON.stringify(row);
  /** The rows nested under one row, at any depth; null when rows do not nest. */
  subRows: ((row: Row) => Row[]) | null = null;
  /** What an open row shows below itself, in a table whose rows do not nest. */
  expand: ((row: Row) => TemplateResult) | null = null;
  /** What the table is sorted by until a header is clicked; unsorted when null. */
  sort: TableSort | null = null;
  empty = 'Nothing in this range';
  filterLabel = 'Filter';

  private filterText = '';
  /** The keys of the rows whose panel is open, in a table whose rows do not nest. */
  private detailOpen = new Set<string>();
  /**
   * The keys of the rows whose children are drawn, in a tree, in the shape
   * TanStack reads as its expanded state. It is replaced rather than changed in
   * place, because the table compares the state it is given against the state it
   * holds by identity.
   */
  private childrenOpen: Record<string, boolean> = {};
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

  private toggleDetail(key: string): void {
    const open = new Set(this.detailOpen);
    if (open.has(key)) open.delete(key);
    else open.add(key);
    this.detailOpen = open;
  }

  private toggleChildren(key: string): void {
    const { [key]: wasOpen, ...rest } = this.childrenOpen;
    this.childrenOpen = wasOpen === true ? rest : { ...rest, [key]: true };
  }

  /** The chevron of one row and, for a row with rows under it, how many there are. */
  private renderChevron(
    open: boolean,
    below: number,
    toggle: () => void,
  ): TemplateResult {
    return html`<sl-icon-button
        name=${open ? 'chevron-down' : 'chevron-right'}
        label=${open ? 'Collapse' : 'Expand'}
        @click=${toggle}
      ></sl-icon-button
      >${below === 0 ? nothing : html`<span class="descendants">${below}</span>`}`;
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
   * One cell. `first` marks the cell that names the row, which in a table whose
   * rows do not nest stays in place while a narrow screen scrolls the rest of the
   * row past it, and which carries the indent of the row's depth. `toggle` is
   * what a tree draws before the name: the row's chevron, or the room one takes.
   */
  private renderCell(
    column: TableColumn<Row>,
    row: Row,
    first: boolean,
    depth: number,
    toggle: TemplateResult | typeof nothing = nothing,
  ): TemplateResult {
    const value = this.renderValue(column, row);
    const target = column.link?.(row) ?? null;
    const linked = target === null ? value : html`<a href=${target}>${value}</a>`;
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
      ${toggle === nothing
        ? linked
        : html`<div class="row-name">
            <span class="row-toggle">${toggle}</span>
            <div class="row-label">${linked}</div>
          </div>`}
    </td>`;
  }

  override render(): TemplateResult {
    const tree = this.subRows !== null;
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
        expanded: this.filterText === '' ? this.childrenOpen : true,
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
    // A row of a flat table opens for what it holds as well as for what it leads
    // to, so a table whose columns do not all fit still opens. A tree opens rows
    // only onto the rows under them.
    const opensDetail = !tree && (this.expand !== null || hidden.length > 0);
    const nests = tree && shown.some((modelRow) => modelRow.subRows.length > 0);
    const width = columns.length + (opensDetail ? 1 : 0);
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
        <table class=${tree ? 'data stats nested' : 'data stats'}>
          <thead>
            <tr>
              ${opensDetail ? html`<th scope="col" class="expander"></th>` : nothing}
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
                  const below = descendantCount(modelRow);
                  const detailOpen = opensDetail && this.detailOpen.has(key);
                  const toggle = !nests
                    ? nothing
                    : below === 0
                      ? html``
                      : this.renderChevron(modelRow.getIsExpanded(), below, () =>
                          this.toggleChildren(key),
                        );
                  return html`<tr
                      class="data-row"
                      @click=${(event: MouseEvent) => this.clicked(event, row)}
                    >
                      ${opensDetail
                        ? html`<td class="expander">
                            ${this.renderChevron(detailOpen, 0, () => this.toggleDetail(key))}
                          </td>`
                        : nothing}
                      ${columns.map((column, index) =>
                        index === 0
                          ? this.renderCell(column, row, true, modelRow.depth, toggle)
                          : this.renderCell(column, row, false, modelRow.depth),
                      )}
                    </tr>
                    ${detailOpen
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
