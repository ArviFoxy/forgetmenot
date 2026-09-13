// One loaded API resource, held by the component that shows it.

export type Loaded<Value> =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'ready'; value: Value }
  | { status: 'failed'; error: Error }
  | { status: 'missing'; error: Error };

/** The status a failed read gets: a 404 is a value the page shows, not a breakdown. */
export type FailureStatus = 'failed' | 'missing';

export interface ResourceOptions {
  /** Decides which failures are reported as `missing` rather than `failed`. */
  classify?: (error: Error) => FailureStatus;
}

/**
 * Loads one resource and keeps its state. A result that arrives after a newer load
 * started is dropped, so a slow answer to an old request cannot replace a new one.
 */
export class Resource<Value> {
  private attempt = 0;
  state: Loaded<Value> = { status: 'idle' };

  constructor(
    private readonly notify: () => void,
    private readonly options: ResourceOptions = {},
  ) {}

  async load(loader: () => Promise<Value>): Promise<void> {
    const attempt = ++this.attempt;
    this.state = { status: 'loading' };
    this.notify();
    try {
      const value = await loader();
      if (attempt !== this.attempt) return;
      this.state = { status: 'ready', value };
    } catch (caught) {
      if (attempt !== this.attempt) return;
      const error = caught instanceof Error ? caught : new Error(String(caught));
      this.state = { status: this.options.classify?.(error) ?? 'failed', error };
    }
    this.notify();
  }

  /** Forgets what was loaded, for a page that no longer has anything to show. */
  reset(): void {
    this.attempt += 1;
    this.state = { status: 'idle' };
    this.notify();
  }

  /** The loaded value, or null while loading or after a failure. */
  get value(): Value | null {
    return this.state.status === 'ready' ? this.state.value : null;
  }
}
