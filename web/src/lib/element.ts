// The app's components render into the page rather than into a shadow root, so the
// stylesheet that styles the document (Pico) reaches the markdown, the tables and
// the form controls inside them.

import { LitElement, html, type TemplateResult } from 'lit';
import type { Loaded } from './resource';

export class PageElement extends LitElement {
  protected override createRenderRoot(): HTMLElement | DocumentFragment {
    return this;
  }
}

/** Shows the load in progress, the failure, or the loaded value. */
export function gate<Value>(
  state: Loaded<Value>,
  ready: (value: Value) => TemplateResult,
  missing?: (error: Error) => TemplateResult,
): TemplateResult {
  switch (state.status) {
    case 'idle':
    case 'loading':
      return html`<p aria-busy="true">Loading</p>`;
    case 'missing':
      return missing?.(state.error) ?? html`<p class="failure" role="alert">${state.error.message}</p>`;
    case 'failed':
      return html`<p class="failure" role="alert">${state.error.message}</p>`;
    case 'ready':
      return ready(state.value);
  }
}
