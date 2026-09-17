import { expect, test } from 'vitest';
import { formatBytes } from '../src/model/units';

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
