import { expect, test, vi } from 'vitest';
import type { Trigger } from '../src/api/types';
import { PatternChecker } from '../src/model/patternCheck';
import { fieldOf, triggerFieldChoices, withField, withMachine } from '../src/model/triggers';

// The source of these expectations is the store's own rule for triggers: a trigger
// without `on` matches every text, `machine` is a plain condition alongside the
// pattern on every trigger, and a pattern is a regex the server compiles.

test('the field list leaves out any, or offers it somewhere other than first', () => {
  expect(triggerFieldChoices[0]).toBe('any');
  expect(triggerFieldChoices).toContain('working_directory');
});

test('a trigger without a field means something other than any', () => {
  expect(fieldOf({ pattern: 'rocket' })).toBe('any');
  expect(fieldOf({ on: 'tool_input', pattern: 'rocket' })).toBe('tool_input');
});

test('choosing any writes an on line anyway', () => {
  const narrowed: Trigger = { on: 'user_message', pattern: 'rocket' };
  expect(withField(narrowed, 'any')).toEqual({ pattern: 'rocket' });
  expect(Object.keys(withField(narrowed, 'any'))).not.toContain('on');
});

test('choosing a field loses the pattern, or drops the machine the trigger names', () => {
  const anywhere: Trigger = { pattern: 'rocket', machine: 'alpha' };
  for (const field of triggerFieldChoices.filter((name) => name !== 'any')) {
    expect(withField(anywhere, field)).toEqual({ on: field, pattern: 'rocket', machine: 'alpha' });
  }
  expect(withField(anywhere, 'any')).toEqual({ pattern: 'rocket', machine: 'alpha' });
});

test('a machine cannot be set from a padded name, or cleared', () => {
  expect(withMachine({ pattern: 'rocket' }, ' beta ')).toEqual({ pattern: 'rocket', machine: 'beta' });
  expect(withMachine({ pattern: 'rocket', machine: 'beta' }, '')).toEqual({ pattern: 'rocket' });
});

test('every keystroke is sent to the server', async () => {
  vi.useFakeTimers();
  const asked: string[] = [];
  const checker = new PatternChecker(async (pattern) => {
    asked.push(pattern);
    return { ok: true };
  }, 100);
  checker.schedule('\\b');
  checker.schedule('\\br');
  checker.schedule('\\broc');
  await vi.advanceTimersByTimeAsync(150);
  expect(asked).toEqual(['\\broc']);
  vi.useRealTimers();
});

test('a pattern the server refused is reported without its message, or asked about twice', async () => {
  vi.useFakeTimers();
  let calls = 0;
  const checker = new PatternChecker(async () => {
    calls += 1;
    return { ok: false, error: 'regex parse error: unclosed group' };
  }, 10);
  checker.schedule('(');
  await vi.advanceTimersByTimeAsync(20);
  expect(checker.messageFor('(')).toBe('regex parse error: unclosed group');
  checker.schedule('(');
  await vi.advanceTimersByTimeAsync(20);
  expect(calls).toBe(1);
  expect(checker.messageFor('\\brocket\\b')).toBeNull();
  vi.useRealTimers();
});

test('an empty pattern is reported as a mistake', async () => {
  vi.useFakeTimers();
  let calls = 0;
  const checker = new PatternChecker(async () => {
    calls += 1;
    return { ok: true };
  }, 10);
  checker.schedule('');
  await vi.advanceTimersByTimeAsync(20);
  expect(calls).toBe(0);
  expect(checker.messageFor('')).toBeNull();
  vi.useRealTimers();
});

test('a pattern that compiles keeps the message of the one before it', async () => {
  vi.useFakeTimers();
  const checker = new PatternChecker(
    async (pattern) => (pattern === '(' ? { ok: false, error: 'unclosed group' } : { ok: true }),
    10,
  );
  checker.schedule('(');
  await vi.advanceTimersByTimeAsync(20);
  checker.schedule('()');
  await vi.advanceTimersByTimeAsync(20);
  expect(checker.messageFor('()')).toBeNull();
  expect(checker.knows('()')).toBe(true);
  vi.useRealTimers();
});
