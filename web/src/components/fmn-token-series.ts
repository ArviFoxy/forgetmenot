// The delivered-tokens line. Observable Plot builds the SVG; this element puts it
// in the page, keeps it the width of its container, and lets a drag across it
// narrow the range the line is drawn over.

import * as Plot from '@observablehq/plot';
import { html, type PropertyDeclarations, type TemplateResult } from 'lit';
import { PageElement } from '../lib/element';
import type { TokenPoint } from '../model/stats';

/** The width the chart is drawn at before the container has been measured. */
const defaultWidth = 640;

const chartHeight = 220;

/** A drag shorter than this is a click, which clears a narrowed range. */
const dragThreshold = 8;

/** The part of the plot a drag is read against, as Plot lays it out. */
const margins = { marginLeft: 48, marginRight: 20, marginTop: 16, marginBottom: 34 };

/** What the plot element offers beyond an SVG: the scales it was drawn with. */
interface Plotted extends HTMLElement {
  scale(name: string): { invert?: (position: number) => number } | undefined;
}

/**
 * Tokens delivered per bucket over time. `from` and `to` are the window the
 * server was asked for, so the line covers the whole of it even where no bucket
 * carries anything; a drag narrows what is drawn to the part dragged over.
 */
export class FmnTokenSeries extends PageElement {
  static override properties: PropertyDeclarations = {
    points: { attribute: false },
    from: { attribute: false },
    to: { attribute: false },
    narrowed: { state: true },
    width: { state: true },
    dragging: { state: true },
  };

  points: TokenPoint[] = [];
  /** The window's start, null when the range is the whole log. */
  from: Date | null = null;
  to: Date | null = null;

  /** The part of the window a drag picked out; null for the whole of it. */
  private narrowed: { from: Date; to: Date } | null = null;
  private width = defaultWidth;
  /** Where the drag started and where the pointer is now, in pixels. */
  private dragging: { start: number; at: number } | null = null;

  private observer: ResizeObserver | null = null;
  private plotted: Plotted | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    if (typeof ResizeObserver === 'function') {
      this.observer = new ResizeObserver(() => this.measure());
      this.observer.observe(this);
    }
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.observer?.disconnect();
    this.observer = null;
  }

  private measure(): void {
    const measured = this.clientWidth;
    if (measured > 0 && Math.abs(measured - this.width) >= 1) this.width = measured;
  }

  /** The window the line is drawn over: the drag's part of it, else the whole. */
  private domain(): [Date, Date] | undefined {
    if (this.narrowed !== null) return [this.narrowed.from, this.narrowed.to];
    if (this.from !== null && this.to !== null) return [this.from, this.to];
    return undefined;
  }

  private buildPlot(): Plotted {
    const domain = this.domain();
    const shown =
      domain === undefined
        ? this.points
        : this.points.filter(
            (point) => point.at >= domain[0] && point.at <= domain[1],
          );
    return Plot.plot({
      width: this.width,
      height: chartHeight,
      ...margins,
      className: 'token-series',
      x: { type: 'utc', label: null, ...(domain === undefined ? {} : { domain }) },
      y: { label: 'tokens', grid: true, zero: true },
      marks: [
        Plot.ruleY([0]),
        Plot.line(shown, { x: 'at', y: 'tokens' }),
        Plot.dot(shown, { x: 'at', y: 'tokens', r: 3, fill: 'currentColor' }),
        Plot.tip(shown, Plot.pointerX({ x: 'at', y: 'tokens' })),
      ],
    }) as Plotted;
  }

  /** What the drawn plot depends on, so that a drag alone does not redraw it. */
  private signature(): string {
    return JSON.stringify([
      this.width,
      this.from?.getTime() ?? null,
      this.to?.getTime() ?? null,
      this.narrowed === null ? null : [this.narrowed.from.getTime(), this.narrowed.to.getTime()],
      this.points.map((point) => [point.at.getTime(), point.tokens]),
    ]);
  }

  private drawn = '';

  override updated(): void {
    const holder = this.querySelector('.chart-plot');
    if (holder === null) return;
    const signature = this.signature();
    if (signature === this.drawn && holder.firstChild !== null) return;
    this.drawn = signature;
    const plotted = this.buildPlot();
    // The labels along the x axis are the times the buckets fall at, which the
    // clock decides; `moment` is what the page marks such a value with.
    for (const labels of plotted.querySelectorAll('g[aria-label="x-axis tick label"]')) {
      labels.classList.add('moment');
    }
    holder.replaceChildren(plotted);
    this.plotted = plotted;
  }

  /** The instant at a pixel across the chart, or null when the scale has no inverse. */
  private instantAt(position: number): Date | null {
    const invert = this.plotted?.scale('x')?.invert;
    if (invert === undefined) return null;
    return new Date(invert(position));
  }

  private startDrag(event: PointerEvent): void {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    this.dragging = { start: event.clientX - box.left, at: event.clientX - box.left };
  }

  private moveDrag(event: PointerEvent): void {
    if (this.dragging === null) return;
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    this.dragging = { start: this.dragging.start, at: event.clientX - box.left };
  }

  /**
   * The end of a drag. A drag long enough to have picked something out narrows
   * the range to it; anything shorter is a click, which puts the whole window
   * back.
   */
  private endDrag(): void {
    const drag = this.dragging;
    this.dragging = null;
    if (drag === null) return;
    if (Math.abs(drag.at - drag.start) < dragThreshold) {
      this.narrowed = null;
      return;
    }
    const first = this.instantAt(Math.min(drag.start, drag.at));
    const last = this.instantAt(Math.max(drag.start, drag.at));
    if (first === null || last === null || Number.isNaN(first.getTime())) return;
    this.narrowed = { from: first, to: last };
  }

  override render(): TemplateResult {
    const drag = this.dragging;
    const left = drag === null ? 0 : Math.min(drag.start, drag.at);
    const width = drag === null ? 0 : Math.abs(drag.at - drag.start);
    return html`<div
      class="chart"
      @pointerdown=${(event: PointerEvent) => this.startDrag(event)}
      @pointermove=${(event: PointerEvent) => this.moveDrag(event)}
      @pointerup=${() => this.endDrag()}
      @pointerleave=${() => this.endDrag()}
    >
      <div class="chart-plot"></div>
      ${drag === null
        ? ''
        : html`<div class="chart-drag" style=${`left: ${left}px; width: ${width}px;`}></div>`}
    </div>`;
  }
}

customElements.define('fmn-token-series', FmnTokenSeries);
