// @vitest-environment jsdom
import { beforeEach, expect, test } from 'vitest';
import { html } from 'lit';
import '../src/components/fmn-editable-table';
import '../src/components/fmn-machine';
import type { EditableColumn, FmnEditableTable, RowEvent } from '../src/components/fmn-editable-table';

// The source of these expectations is what the table promises: it shows a row per
// entry with the caller's controls in it, it asks the owner of the data to add and
// remove rows rather than doing it itself, and a machine with no name reads as Any.

interface Row {
  name: string;
}

const columns: EditableColumn<Row>[] = [
  { label: 'Name', cell: (row) => html`<input class="cell-control" value=${row.name} />` },
  { label: 'Note', cell: () => html`<span>note</span>` },
];

async function table(rows: Row[]): Promise<FmnEditableTable<Row>> {
  const element = document.createElement('fmn-editable-table') as FmnEditableTable<Row>;
  element.columns = columns;
  element.rows = rows;
  element.addLabel = 'Add row';
  element.removeLabel = 'Remove row';
  element.emptyText = 'Nothing yet';
  document.body.append(element);
  await (element as unknown as { updateComplete: Promise<unknown> }).updateComplete;
  return element;
}

beforeEach(() => {
  document.body.innerHTML = '';
});

test('the head names something other than the columns it was given', async () => {
  const element = await table([{ name: 'first' }]);
  const heads = [...element.querySelectorAll('[role="columnheader"]')].map((cell) =>
    cell.textContent?.trim(),
  );
  expect(heads).toEqual(['Name', 'Note', '']);
});

test('a row is drawn per entry, with the caller control in the cell', async () => {
  const element = await table([{ name: 'first' }, { name: 'second' }]);
  const rows = element.querySelectorAll('.editable-row:not(.editable-head):not(.editable-add)');
  expect(rows).toHaveLength(2);
  const values = [...element.querySelectorAll('input')].map((input) => input.value);
  expect(values).toEqual(['first', 'second']);
});

test('an empty table hides that it is empty', async () => {
  const element = await table([]);
  expect(element.querySelector('.empty')?.textContent?.trim()).toBe('Nothing yet');
  expect(element.querySelector('sl-button')).not.toBeNull();
});

test('the table adds and removes rows itself instead of asking', async () => {
  const element = await table([{ name: 'first' }, { name: 'second' }]);
  const seen: { name: string; index: number }[] = [];
  element.addEventListener('fmn-row-add', (event) =>
    seen.push({ name: 'add', index: (event as CustomEvent<RowEvent>).detail.index }),
  );
  element.addEventListener('fmn-row-remove', (event) =>
    seen.push({ name: 'remove', index: (event as CustomEvent<RowEvent>).detail.index }),
  );

  const removes = element.querySelectorAll('.editable-actions sl-icon-button');
  (removes[1] as HTMLElement).click();
  (element.querySelector('.editable-add sl-button') as HTMLElement).click();

  expect(seen).toEqual([
    { name: 'remove', index: 1 },
    { name: 'add', index: 2 },
  ]);
  // The rows it was given are untouched; the page that owns them decides.
  expect(element.rows).toHaveLength(2);
});

test('every remove control carries the same label, so one row cannot be told from another', async () => {
  const element = await table([{ name: 'first' }, { name: 'second' }]);
  const labels = [...element.querySelectorAll('.editable-actions sl-icon-button')].map((button) =>
    button.getAttribute('label'),
  );
  expect(labels).toEqual(['Remove row 1', 'Remove row 2']);
});

test('a machine with no name reads as a name', async () => {
  const element = document.createElement('fmn-machine-value');
  element.setAttribute('machine', '');
  document.body.append(element);
  await (element as unknown as { updateComplete: Promise<unknown> }).updateComplete;

  const any = element.querySelector('.machine-any');
  expect(any?.textContent?.trim()).toBe('Any');
  expect(any?.tagName).toBe('EM');
  expect(element.querySelector('.machine-name')).toBeNull();
});

test('a machine that has a name is shown as though it had none', async () => {
  const element = document.createElement('fmn-machine-value');
  element.setAttribute('machine', 'alpha');
  document.body.append(element);
  await (element as unknown as { updateComplete: Promise<unknown> }).updateComplete;

  expect(element.querySelector('.machine-name')?.textContent?.trim()).toBe('alpha');
  expect(element.querySelector('.machine-any')).toBeNull();
});
