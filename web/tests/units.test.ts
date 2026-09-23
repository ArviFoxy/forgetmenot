import { expect, test } from 'vitest';
import { formatAgo, formatBytes, formatTokens } from '../src/model/units';

// The source of these expectations is the unit table decided in issue #12: 1024
// per step, a plain byte count below 1024, one decimal above it, so 0 B, 1023 B,
// 1.0 KB at 1024, 1.5 KB at 1536, 24.6 KB at 25205, 1.2 MB at 1258291 and
// 2.5 GB at 2684354560.

test('a count below one kilobyte is scaled or given a decimal instead of printed as bytes', () => {
  expect(formatBytes(0)).toBe('0 B');
  expect(formatBytes(1023)).toBe('1023 B');
});

test('a count is divided by 1000 rather than 1024, or loses its decimal', () => {
  expect(formatBytes(1024)).toBe('1.0 KB');
  expect(formatBytes(1536)).toBe('1.5 KB');
  expect(formatBytes(25205)).toBe('24.6 KB');
  expect(formatBytes(1258291)).toBe('1.2 MB');
  expect(formatBytes(2684354560)).toBe('2.5 GB');
});

// A count whose decimal rounds up to 1024.0 of a unit is shown in the next unit,
// since 1024.0 KB is not the unit that fits it.
test('a count whose decimal rounds up to 1024.0 keeps the smaller unit', () => {
  expect(formatBytes(1048530)).toBe('1.0 MB');
  expect(formatBytes(1073689396)).toBe('1.0 GB');
});

// The source of these expectations is the form the dashboard shows a token
// count in: three digits at most, so a plain count below 1000, one decimal while
// the scaled figure is under ten, and a whole number above it, in steps of 1000.

test('a token count below a thousand is scaled or given a decimal instead of printed whole', () => {
  expect(formatTokens(0)).toBe('0');
  expect(formatTokens(1)).toBe('1');
  expect(formatTokens(999)).toBe('999');
});

test('a token count keeps a decimal past ten of a unit, or loses it below ten', () => {
  expect(formatTokens(1000)).toBe('1.0k');
  expect(formatTokens(1200)).toBe('1.2k');
  expect(formatTokens(9949)).toBe('9.9k');
  expect(formatTokens(9950)).toBe('10k');
  expect(formatTokens(34000)).toBe('34k');
  expect(formatTokens(999499)).toBe('999k');
});

test('a token count is divided by 1024 rather than 1000, or misses the step to the next unit', () => {
  expect(formatTokens(1024)).toBe('1.0k');
  expect(formatTokens(1500000)).toBe('1.5M');
  expect(formatTokens(34000000)).toBe('34M');
  expect(formatTokens(2500000000)).toBe('2.5G');
});

// A figure that rounds to 1000 of a unit is shown in the next unit, since 1000k is
// not the unit that fits it.
test('a token count that rounds to a thousand of a unit keeps the smaller unit', () => {
  expect(formatTokens(999500)).toBe('1.0M');
  expect(formatTokens(999500000)).toBe('1.0G');
});

// The source of these expectations is the form the contexts page shows an age in:
// `just now` under a minute, then whole minutes, hours and days, each counted down
// to the whole unit, so 59 s is `just now`, 60 s `1 min`, 59 min 59 s `59 min`,
// one hour `1 h`, 23 h 59 min `23 h` and one day `1 d`.

const now = new Date('2026-09-23T12:00:00Z');

/** The instant `seconds` before `now`, as the server writes one. */
function before(seconds: number): string {
  return new Date(now.getTime() - seconds * 1000).toISOString();
}

test('an age is moved to the next unit before it fills it, or rounded up to it', () => {
  expect(formatAgo(before(59), now)).toBe('just now');
  expect(formatAgo(before(3599), now)).toBe('59 min');
  expect(formatAgo(before(86_399), now)).toBe('23 h');
});

test('an age that fills a unit is still counted in the unit below', () => {
  expect(formatAgo(before(60), now)).toBe('1 min');
  expect(formatAgo(before(3600), now)).toBe('1 h');
  expect(formatAgo(before(86_400), now)).toBe('1 d');
  expect(formatAgo(before(3 * 86_400 + 3600), now)).toBe('3 d');
});

test('an instant after now reads as a negative age instead of just now', () => {
  expect(formatAgo(before(-30), now)).toBe('just now');
});

test('an age in another time zone offset is read as a different instant', () => {
  expect(formatAgo('2026-09-23T13:55:00+02:00', now)).toBe('5 min');
});
