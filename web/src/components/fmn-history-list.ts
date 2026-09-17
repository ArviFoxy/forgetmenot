import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import type { Commit } from '../api/types';
import { PageElement } from '../lib/element';

/**
 * A list of commits, newest first. `href` decides what a row opens, so the same
 * table serves one document's history and the whole store's; a list without one
 * shows the titles as text.
 */
export class FmnHistoryList extends PageElement {
  static override properties: PropertyDeclarations = {
    commits: { attribute: false },
    href: { attribute: false },
  };

  commits: Commit[] = [];
  href: ((commit: Commit) => string) | null = null;

  override render(): TemplateResult {
    const link = this.href;
    return html`<div class="table-wrap">
      <table class="data">
        <thead>
          <tr>
            <th scope="col">Title</th>
            <th scope="col">Time</th>
            <th scope="col">Author</th>
            <th scope="col">Commit</th>
          </tr>
        </thead>
        <tbody>
          ${this.commits.map(
            (commit) => html`<tr>
              <td>${link === null ? commit.title : html`<a href=${link(commit)}>${commit.title}</a>`}</td>
              <td class="nowrap">${commit.time}</td>
              <td>${commit.author}</td>
              <td><code>${commit.oid.slice(0, 10)}</code></td>
            </tr>`,
          )}
        </tbody>
      </table>
    </div>`;
  }
}

customElements.define('fmn-history-list', FmnHistoryList);
