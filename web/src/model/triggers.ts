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
 * A path means different things on different machines while a message does not, so
 * only these two fields may be narrowed to one machine.
 */
export function takesMachine(field: TriggerField): boolean {
  return field === 'any' || field === 'working_directory';
}

/**
 * The trigger with its field changed. `any` is written by leaving `on` out, so that
 * the file keeps the shape the server writes; a machine that the new field cannot
 * carry goes with it.
 */
export function withField(trigger: Trigger, field: TriggerField): Trigger {
  const { machine, pattern } = trigger;
  const kept = takesMachine(field) && machine !== undefined && machine !== '' ? { machine } : {};
  return field === 'any' ? { pattern, ...kept } : { on: field, pattern, ...kept };
}

/**
 * How a machine is shown: the name, or `Any` for a trigger that names none. The two
 * are told apart by `any`, because a machine could itself be called Any.
 */
export function machineDisplay(machine: string | undefined): { text: string; any: boolean } {
  const named = (machine ?? '').trim();
  return named === '' ? { text: 'Any', any: true } : { text: named, any: false };
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
