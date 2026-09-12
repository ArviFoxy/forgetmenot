import { html, type TemplateResult } from 'lit';
import { PageElement } from '../lib/element';
import '../components/fmn-trigger-test';

/** The trigger test on its own page. */
export class FmnTriggerTestView extends PageElement {
  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <h1>Trigger test</h1>
      </header>
      <fmn-trigger-test heading="Input" idPrefix="page-test"></fmn-trigger-test>
    `;
  }
}

customElements.define('fmn-trigger-test-view', FmnTriggerTestView);
