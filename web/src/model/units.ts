// Sizes and ages as a reader of the page sees them.

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

// 1000-based units, the way a count of tokens is read aloud. Tokens are the unit
// below the first of these.
const unitsAboveTokens = ['k', 'M', 'G'];

/**
 * A token count as a figure of at most three digits: the plain count below 1000,
 * then one decimal while the scaled figure is under ten (`1.2k`, `1.5M`) and a
 * whole number above it (`34k`). A figure that rounds to 1000 of a unit moves up
 * to the next one, so far as G.
 */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return String(tokens);
  let scaled = tokens / 1000;
  let unit = 0;
  while (scaled >= 1000 && unit + 1 < unitsAboveTokens.length) {
    scaled /= 1000;
    unit += 1;
  }
  // 9.95 and up would print as `10.0`, which is the whole-number form with a
  // decimal stuck on it, so the change of form happens there rather than at 10.
  if (scaled < 9.95) {
    const tenths = Math.round(scaled * 10);
    return `${Math.floor(tenths / 10)}.${tenths % 10}${unitsAboveTokens[unit]}`;
  }
  const whole = Math.round(scaled);
  if (whole >= 1000 && unit + 1 < unitsAboveTokens.length) return `1.0${unitsAboveTokens[unit + 1]}`;
  return `${whole}${unitsAboveTokens[unit]}`;
}

const minute = 60_000;
const hour = 60 * minute;
const day = 24 * hour;

/**
 * How long before `now` the instant `iso` was, in the largest whole unit it
 * fills: `just now` under a minute (and for an instant after `now`), then
 * `N min` under an hour, `N h` under a day, and `N d` from there on.
 */
export function formatAgo(iso: string, now: Date): string {
  const elapsed = now.getTime() - new Date(iso).getTime();
  if (elapsed < minute) return 'just now';
  if (elapsed < hour) return `${Math.floor(elapsed / minute)} min`;
  if (elapsed < day) return `${Math.floor(elapsed / hour)} h`;
  return `${Math.floor(elapsed / day)} d`;
}
