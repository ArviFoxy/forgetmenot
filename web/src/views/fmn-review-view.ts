import { html, type TemplateResult } from 'lit';
import { api } from '../api/client';
import type { ReviewReport } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { paths } from '../routes';

/** The store's validation errors, and the critical memories only the global scope carries. */
export class FmnReviewView extends PageElement {
  private readonly report = new Resource<ReviewReport>(() => this.requestUpdate());
  private started = false;

  override updated(): void {
    if (this.started) return;
    this.started = true;
    void this.report.load(() => api.review());
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <h1>Review</h1>
      </header>
      ${gate(
        this.report.state,
        (report) => html`
          <h2>Errors</h2>
          ${report.errors.length === 0
            ? html`<p class="empty">No errors</p>`
            : html`<figure>
                <table>
                  <thead>
                    <tr>
                      <th scope="col">Path</th>
                      <th scope="col">Message</th>
                    </tr>
                  </thead>
                  <tbody>
                    ${report.errors.map(
                      (error) => html`<tr>
                        <td><code>${error.path}</code></td>
                        <td>${error.message}</td>
                      </tr>`,
                    )}
                  </tbody>
                </table>
              </figure>`}

          <h2>Critical memories in the global scope only</h2>
          ${report.global_only_critical.length === 0
            ? html`<p class="empty">None</p>`
            : html`<figure>
                <table>
                  <thead>
                    <tr>
                      <th scope="col">Memory</th>
                    </tr>
                  </thead>
                  <tbody>
                    ${report.global_only_critical.map(
                      (id) => html`<tr>
                        <td><a href=${paths.memory(id)}>${id}</a></td>
                      </tr>`,
                    )}
                  </tbody>
                </table>
              </figure>`}
        `,
      )}
    `;
  }
}

customElements.define('fmn-review-view', FmnReviewView);
