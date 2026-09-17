// @vitest-environment jsdom
import { beforeEach, expect, test, vi } from 'vitest';
import type { SettingsDoc } from '../src/api/types';
import {
  changedKeys,
  controlFor,
  parseNumberOrOff,
  settingRows,
  settingText,
} from '../src/model/settings';

// The source of these expectations is what the settings page promises: it is built
// from the schema the server sends, every type it defines has a control, and a write
// the server has moved past is shown as a conflict rather than swallowed.

const doc: SettingsDoc = {
  settings: { reminder_tokens: 20000, interrupt_exempt_tools: ['Read'], characters_per_token: 4 },
  version: 'a'.repeat(40),
  schema: [
    {
      key: 'reminder_tokens',
      type: 'integer or null',
      default: 30000,
      description: 'Deliver everything that applies again after this many context tokens',
    },
    {
      key: 'interrupt_on_critical',
      type: 'bool',
      default: true,
      description: 'Hold a tool call when a critical memory is due',
    },
    {
      key: 'interrupt_exempt_tools',
      type: 'list of strings',
      default: [],
      description: 'Tool names that are never held',
    },
    {
      key: 'tool_result_match_limit',
      type: 'integer',
      default: 262144,
      description: 'Bytes of a tool result matched against triggers',
    },
    {
      key: 'characters_per_token',
      type: 'number',
      default: 3.5,
      description: 'Characters one token is worth',
    },
  ],
};

test('a type the schema uses has no control on the page', () => {
  expect(controlFor('integer or null')).toBe('number-or-off');
  expect(controlFor('integer')).toBe('number');
  expect(controlFor('number')).toBe('number');
  expect(controlFor('bool')).toBe('switch');
  expect(controlFor('list of strings')).toBe('tags');
  for (const row of doc.schema) expect(controlFor(row.type)).not.toBeNull();
});

test('a key the file does not set is shown as unset instead of as its default', () => {
  const rows = settingRows(doc);
  expect(rows.map((row) => row.key)).toEqual([
    'reminder_tokens',
    'interrupt_on_critical',
    'interrupt_exempt_tools',
    'tool_result_match_limit',
    'characters_per_token',
  ]);
  expect(rows[0]).toMatchObject({ value: 20000, isDefault: false, control: 'number-or-off' });
  expect(rows[1]).toMatchObject({ value: true, isDefault: true, control: 'switch' });
  expect(rows[2]).toMatchObject({ value: ['Read'], isDefault: false, control: 'tags' });
});

test('an empty number field reads as zero rather than as off', () => {
  expect(parseNumberOrOff('')).toBeNull();
  expect(parseNumberOrOff('  ')).toBeNull();
  expect(parseNumberOrOff('20000')).toBe(20000);
  expect(parseNumberOrOff('12.7')).toBe(12);
  expect(parseNumberOrOff('none')).toBeNull();
});

// The source of this expectation is the settings schema: `characters_per_token` is
// typed `number` and its default is 3.5, so a field that truncates writes 3 and
// changes the setting the reader was looking at.
test('a fraction typed into a number setting is truncated, or kept in one that counts whole things', () => {
  expect(parseNumberOrOff('3.5', 'number')).toBe(3.5);
  expect(parseNumberOrOff('3.5', 'integer')).toBe(3);
  expect(parseNumberOrOff('3.5', 'integer or null')).toBe(3);
  expect(parseNumberOrOff('', 'number')).toBeNull();
});

test('a value keeps its shape when it is written out for a reader', () => {
  expect(settingText(null)).toBe('off');
  expect(settingText(true)).toBe('on');
  expect(settingText(false)).toBe('off');
  expect(settingText([])).toBe('none');
  expect(settingText(['Read', 'Grep'])).toBe('Read, Grep');
  expect(settingText(42)).toBe('42');
});

test('a value typed back to what it was still counts as a change', () => {
  expect(changedKeys(doc, {})).toEqual([]);
  expect(changedKeys(doc, { reminder_tokens: 20000 })).toEqual([]);
  expect(changedKeys(doc, { reminder_tokens: null })).toEqual(['reminder_tokens']);
  expect(changedKeys(doc, { interrupt_exempt_tools: ['Read', 'Grep'] })).toEqual([
    'interrupt_exempt_tools',
  ]);
});

const putSetting = vi.fn();

vi.mock('../src/api/client', () => ({
  RequestFailed: class RequestFailed extends Error {},
  MissingCommitMessage: class MissingCommitMessage extends Error {},
  api: {
    settings: () => Promise.resolve(doc),
    putSetting: (key: string, request: unknown) => putSetting(key, request) as unknown,
  },
}));

await import('../src/views/fmn-settings-view');

async function settle(element: HTMLElement & { updateComplete?: Promise<unknown> }): Promise<void> {
  for (let round = 0; round < 20; round += 1) {
    await element.updateComplete;
    await new Promise((done) => setTimeout(done, 0));
  }
}

beforeEach(() => {
  document.body.innerHTML = '';
  putSetting.mockReset();
});

test('the page is built from a list of keys of its own rather than from the schema', async () => {
  const element = document.createElement('fmn-settings-view');
  document.body.append(element);
  await settle(element);

  const keys = [...element.querySelectorAll('.setting-name code')].map((code) =>
    code.textContent?.trim(),
  );
  expect(keys).toEqual([
    'reminder_tokens',
    'interrupt_on_critical',
    'interrupt_exempt_tools',
    'tool_result_match_limit',
    'characters_per_token',
  ]);
  expect(element.querySelectorAll('sl-switch')).toHaveLength(1);
  expect(element.querySelectorAll('fmn-tag-field')).toHaveLength(1);
  expect(element.querySelectorAll('sl-input[type="number"]')).toHaveLength(3);
  // Nothing has changed yet, so there is nothing to commit.
  expect(element.querySelector('fmn-commit-bar')).toBeNull();
});

test('a write the server has moved past is reported as though it had been written', async () => {
  const current: SettingsDoc = { ...doc, settings: { reminder_tokens: 9000 }, version: 'b'.repeat(40) };
  putSetting.mockResolvedValue({ kind: 'conflict', conflict: { current } });

  const element = document.createElement('fmn-settings-view');
  document.body.append(element);
  await settle(element);

  const field = element.querySelector('sl-input[type="number"]') as HTMLInputElement;
  field.value = '15000';
  field.dispatchEvent(new CustomEvent('sl-input', { bubbles: true }));
  await settle(element);

  const bar = element.querySelector('fmn-commit-bar');
  expect(bar).not.toBeNull();
  bar?.dispatchEvent(new CustomEvent('fmn-message-change', { detail: { message: 'fewer tokens' } }));
  bar?.dispatchEvent(new CustomEvent('fmn-save'));
  await settle(element);

  expect(putSetting).toHaveBeenCalledWith('reminder_tokens', {
    value: 15000,
    base_version: 'a'.repeat(40),
    author: 'wiki',
    message: 'fewer tokens',
  });
  const conflict = element.querySelector('.conflict');
  expect(conflict).not.toBeNull();
  expect(conflict?.textContent).toContain('reminder_tokens: 9000');
  expect(conflict?.textContent).toContain('reminder_tokens: 15000');
});

test('the number control cuts the fraction off a setting that is a fraction', async () => {
  putSetting.mockResolvedValue({ kind: 'written', response: { commit_oid: 'c', version: 'b' } });

  const element = document.createElement('fmn-settings-view');
  document.body.append(element);
  await settle(element);

  const fields = [...element.querySelectorAll('sl-input[type="number"]')] as HTMLInputElement[];
  const field = fields.at(-1);
  expect(field, 'the number setting must have a field').not.toBeUndefined();
  field!.value = '3.5';
  field!.dispatchEvent(new CustomEvent('sl-input', { bubbles: true }));
  await settle(element);

  const bar = element.querySelector('fmn-commit-bar');
  expect(bar, 'a changed setting must offer to be saved').not.toBeNull();
  bar?.dispatchEvent(
    new CustomEvent('fmn-message-change', { detail: { message: 'count characters' } }),
  );
  bar?.dispatchEvent(new CustomEvent('fmn-save'));
  await settle(element);

  expect(putSetting).toHaveBeenCalledWith(
    'characters_per_token',
    expect.objectContaining({ value: 3.5 }),
  );
});
