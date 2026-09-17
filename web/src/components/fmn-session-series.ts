// The events of one context: the context size Claude Code reported, and the tokens
// each answer carried. Plot has one y scale per plot, so the two figures are drawn
// as two panels stacked on one x scale, which is the order the events happened in.

import * as Plot from '@observablehq/plot';
import { html, type PropertyDeclarations, type TemplateResult } from 'lit';
import { api } from '../api/client';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import type { SessionEventPoint, StatsQuery } from '../api/types';
import { sessionPoints, type SessionPoint } from '../model/stats';

const defaultWidth = 560;

const panelHeight = 170;

// A rotated tick label is clipped to what the bottom margin holds, and the event
// names are long, so the margin is the room the longest of them needs.
const margins = { marginLeft: 52, marginRight: 16, marginTop: 18, marginBottom: 74 };

/** One event with its place in the order, which is what both panels put on x. */
interface Placed extends SessionPoint {
  order: number;
}

function placed(points: SessionPoint[]): Placed[] {
  return points.map((point, order) => ({ ...point, order }));
}

/**
 * The per-event chart of one context, loaded when the row it sits under is opened.
 * The reported size is Claude Code's own measurement and is drawn beside the
 * estimate, never fitted to it.
 */
export class FmnSessionSeries extends PageElement {
  static override properties: PropertyDeclarations = {
    sessionKey: { type: String },
    window: { attribute: false },
    width: { state: true },
  };

  sessionKey = '';
  /** The window the page is looking at; the events outside it are not drawn. */
  window: StatsQuery = {};

  private width = defaultWidth;
  private readonly events = new Resource<SessionEventPoint[]>(() => this.requestUpdate());
  private loaded = '';
  private observer: ResizeObserver | null = null;
  private drawn = '';

  override connectedCallback(): void {
    super.connectedCallback();
    if (typeof ResizeObserver === 'function') {
      this.observer = new ResizeObserver(() => {
        const measured = this.clientWidth;
        if (measured > 0 && Math.abs(measured - this.width) >= 1) this.width = measured;
      });
      this.observer.observe(this);
    }
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.observer?.disconnect();
    this.observer = null;
  }

  private panels(points: Placed[]): HTMLElement {
    const shared = {
      width: this.width,
      ...margins,
      x: {
        type: 'band' as const,
        label: null,
        domain: points.map((point) => point.order),
        tickFormat: (order: number) => points[order]?.event ?? '',
        tickRotate: -30,
      },
    };
    const sizes = points.filter((point) => point.contextTokens !== null);
    const size = Plot.plot({
      ...shared,
      height: panelHeight,
      className: 'session-size',
      y: { label: 'context tokens', grid: true, zero: true },
      marks: [
        Plot.ruleY([0]),
        Plot.line(sizes, { x: 'order', y: 'contextTokens' }),
        Plot.dot(sizes, { x: 'order', y: 'contextTokens', r: 3, fill: 'currentColor' }),
        Plot.tip(sizes, Plot.pointerX({ x: 'order', y: 'contextTokens' })),
      ],
    });
    const delivered = Plot.plot({
      ...shared,
      height: panelHeight,
      className: 'session-tokens',
      y: { label: 'delivered tokens', grid: true, zero: true },
      marks: [
        Plot.ruleY([0]),
        Plot.barY(points, { x: 'order', y: 'tokens' }),
        Plot.tip(points, Plot.pointerX({ x: 'order', y: 'tokens' })),
      ],
    });
    const holder = document.createElement('div');
    holder.className = 'chart-panels';
    holder.append(size, delivered);
    return holder;
  }

  override updated(): void {
    const wanted = `${this.sessionKey}?${JSON.stringify(this.window)}`;
    if (this.sessionKey !== '' && this.loaded !== wanted) {
      this.loaded = wanted;
      void this.events.load(() => api.sessionSeries(this.sessionKey, this.window));
    }
    const holder = this.querySelector('.chart-plot');
    const rows = this.events.value;
    if (holder === null || rows === null) return;
    const signature = JSON.stringify([this.width, rows]);
    if (signature === this.drawn && holder.firstChild !== null) return;
    this.drawn = signature;
    holder.replaceChildren(this.panels(placed(sessionPoints(rows))));
  }

  override render(): TemplateResult {
    return gate(
      this.events.state,
      (rows) =>
        rows.length === 0
          ? html`<p class="empty">Nothing in this range</p>`
          : html`<div class="chart"><div class="chart-plot"></div></div>`,
    );
  }
}

customElements.define('fmn-session-series', FmnSessionSeries);
