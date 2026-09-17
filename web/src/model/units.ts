// Sizes as a reader of the page sees them.

// 1024-based units, the way a file manager counts them. Bytes are the unit below
// the first of these.
const unitsAboveBytes = ['KB', 'MB', 'GB'];

/**
 * A byte count in the unit that fits it: a plain count below 1024, then one
 * decimal in KB, MB or GB (`24.6 KB`, `1.2 MB`). A count that would print as
 * `1024.0` of a unit moves up to the next one, so far as GB.
 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let scaled = bytes / 1024;
  let unit = 0;
  let tenths = Math.round(scaled * 10);
  while (tenths >= 10240 && unit + 1 < unitsAboveBytes.length) {
    scaled /= 1024;
    unit += 1;
    tenths = Math.round(scaled * 10);
  }
  return `${Math.floor(tenths / 10)}.${tenths % 10} ${unitsAboveBytes[unit]}`;
}
