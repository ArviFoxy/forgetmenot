// What the settings page draws, decided from the schema the server sends rather
// than from a list of keys written here: a key added to the server appears on the
// page with its description and a control that fits its type.

import type { SettingSchemaRow, SettingType, SettingValue, SettingsDoc } from '../api/types';

/** The control a value of this type is changed with. */
export type ControlKind = 'number-or-off' | 'number' | 'switch' | 'tags';

const controls: Record<SettingType, ControlKind> = {
  'integer or null': 'number-or-off',
  integer: 'number',
  bool: 'switch',
  'list of strings': 'tags',
};

/**
 * The control for a type. An unknown type is shown as text rather than guessed
 * at, because offering a number field for something that is not a number would
 * write a value the server refuses.
 */
export function controlFor(type: SettingType): ControlKind | null {
  return controls[type] ?? null;
}

export interface SettingRow {
  key: string;
  description: string;
  type: SettingType;
  control: ControlKind | null;
  /** The value in force: the file's when it sets the key, else the default. */
  value: SettingValue;
  isDefault: boolean;
}

/** One row per key of the schema, in the order the server lists them. */
export function settingRows(doc: SettingsDoc): SettingRow[] {
  return doc.schema.map((row: SettingSchemaRow) => {
    const set = Object.prototype.hasOwnProperty.call(doc.settings, row.key);
    return {
      key: row.key,
      description: row.description,
      type: row.type,
      control: controlFor(row.type),
      value: set ? (doc.settings[row.key] ?? null) : row.default,
      isDefault: !set,
    };
  });
}

/** A number typed into a field, or null for a field left empty, which is "off". */
export function parseNumberOrOff(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  const value = Number(trimmed);
  return Number.isFinite(value) ? Math.trunc(value) : null;
}

/** A value as a line of text, for a field and for the two sides of a conflict. */
export function settingText(value: SettingValue): string {
  if (value === null) return 'off';
  if (typeof value === 'boolean') return value ? 'on' : 'off';
  if (Array.isArray(value)) return value.length === 0 ? 'none' : value.join(', ');
  return String(value);
}

/** Whether what the page holds differs from what the server sent. */
export function changedKeys(
  doc: SettingsDoc,
  edited: Record<string, SettingValue>,
): string[] {
  const rows = settingRows(doc);
  return rows
    .filter((row) => Object.prototype.hasOwnProperty.call(edited, row.key))
    .filter((row) => JSON.stringify(edited[row.key]) !== JSON.stringify(row.value))
    .map((row) => row.key);
}
