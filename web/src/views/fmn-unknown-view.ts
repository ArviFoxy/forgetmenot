import { html, type TemplateResult } from 'lit';
import { PageElement } from '../lib/element';
import { currentPath } from '../navigation';
import { paths } from '../routes';

/** An address the app has no view for. */
export class FmnUnknownView extends PageElement {
  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <h1>No view for ${currentPath()}</h1>
      </header>
      <p><a href=${paths.home()}>Memories and scopes</a></p>
    `;
  }
}

customElements.define('fmn-unknown-view', FmnUnknownView);
