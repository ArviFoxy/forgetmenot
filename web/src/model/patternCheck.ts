// Checking a pattern while it is typed: one request per pause, one answer kept per
// pattern. The server compiles the pattern with the same regex engine the store
// uses, so what it says is what a save would say.

import type { PatternValidity } from '../api/types';

export type ValidatePattern = (pattern: string) => Promise<PatternValidity>;

/**
 * Holds what the server said about each pattern it was asked about.
 *
 * A pattern is asked about once: the answer only depends on the pattern, so typing
 * back to a pattern already checked shows its message without another request. The
 * empty pattern is never sent; an empty field is not a mistake yet.
 */
export class PatternChecker {
  /** The message for a pattern that does not compile, `null` for one that does. */
  private readonly answers = new Map<string, string | null>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private pending: string | null = null;

  constructor(
    private readonly validate: ValidatePattern,
    /** How long the typing has to stop before the pattern is sent. */
    private readonly delay = 250,
    /** Called when an answer arrives, so the page can draw it. */
    private readonly onAnswer: () => void = () => {},
  ) {}

  /** What is known about this pattern right now, without asking again. */
  messageFor(pattern: string): string | null {
    return this.answers.get(pattern) ?? null;
  }

  /** Whether this pattern has been answered, so a field knows to stay quiet. */
  knows(pattern: string): boolean {
    return this.answers.has(pattern);
  }

  /** Asks about this pattern once the typing stops. */
  schedule(pattern: string): void {
    if (pattern === '' || this.answers.has(pattern)) return;
    this.pending = pattern;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      const asked = this.pending;
      this.pending = null;
      if (asked !== null) void this.ask(asked);
    }, this.delay);
  }

  /** Drops a scheduled request, for a field that is gone. */
  cancel(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.pending = null;
  }

  private async ask(pattern: string): Promise<void> {
    try {
      const validity = await this.validate(pattern);
      this.answers.set(pattern, validity.ok ? null : (validity.error ?? 'not a valid pattern'));
    } catch {
      // A failed request says nothing about the pattern; the save still reports it.
      return;
    }
    this.onAnswer();
  }
}
