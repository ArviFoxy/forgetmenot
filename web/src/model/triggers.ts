import type { MemoryKind, MemorySource, TriggerField } from '../api/types';

/** The six strings the harness supplies, in the order the documentation lists them. */
export const triggerFields: TriggerField[] = [
  'user_message',
  'assistant_message',
  'tool_name',
  'tool_input',
  'tool_result',
  'working_directory',
];

export const memoryKinds: MemoryKind[] = ['critical', 'knowledge'];
export const memorySources: MemorySource[] = ['user', 'assistant'];

/** A comma-separated list of ids, as the id fields take them. */
export function parseIdList(text: string): string[] {
  return text
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part !== '');
}
