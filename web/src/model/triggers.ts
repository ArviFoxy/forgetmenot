import type { MemoryKind, MemorySource, Trigger, TriggerField } from '../api/types';

/** The six strings the harness supplies, in the order the documentation lists them. */
export const triggerFields: TriggerField[] = [
  'user_message',
  'assistant_message',
  'tool_name',
  'tool_input',
  'tool_result',
  'working_directory',
];

/**
 * What a field select offers. `any` comes first and is the default, because a
 * trigger that names no field matches every text, which is the plain case; the six
 * texts narrow it.
 */
export const triggerFieldChoices: TriggerField[] = ['any', ...triggerFields];

/** The field a trigger matches on: `any` when the file says nothing. */
export function fieldOf(trigger: Trigger): TriggerField {
  return trigger.on ?? 'any';
}

/**
 * The trigger with its field changed. `any` is written by leaving `on` out, so that
 * the file keeps the shape the server writes; the machine is a condition of its own
 * and survives the change of field.
 */
export function withField(trigger: Trigger, field: TriggerField): Trigger {
  const { machine, pattern } = trigger;
  const kept = machine !== undefined && machine !== '' ? { machine } : {};
  return field === 'any' ? { pattern, ...kept } : { on: field, pattern, ...kept };
}

/** The trigger with its machine changed; an empty name is no machine at all. */
export function withMachine(trigger: Trigger, machine: string): Trigger {
  const { on, pattern } = trigger;
  const named = machine.trim();
  return { ...(on === undefined ? {} : { on }), pattern, ...(named === '' ? {} : { machine: named }) };
}

export const memoryKinds: MemoryKind[] = ['critical', 'knowledge'];
export const memorySources: MemorySource[] = ['user', 'assistant'];

/** A comma-separated list of ids, as the id fields take them. */
export function parseIdList(text: string): string[] {
  return text
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part !== '');
}
