import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { Commit, CommitFile, CommitFiles } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-diff';
import '../components/fmn-history-list';

export type StoreHistoryMode = 'list' | 'commit';

/** The page of the document a file holds, or null when it holds none. */
function fileTarget(file: CommitFile): string | null {
  if (file.memory_id !== null) return paths.memory(file.memory_id);
  if (file.scope_id !== null) return paths.scope(file.scope_id);
  return null;
}

/**
 * The store's commits, and one commit with every file it changed. Both are this
 * one element, so a row of the list and the page it opens report a commit the
 * same way.
 */
export class FmnHistoryView extends PageElement {
  static override properties: PropertyDeclarations = {
    mode: { type: String },
    oid: { type: String },
    older: { state: true },
    loadingOlder: { state: true },
    olderFailure: { state: true },
  };

  mode: StoreHistoryMode = 'list';
  oid = '';

  private readonly page = new Resource<Commit[]>(() => this.requestUpdate());
  private readonly commit = new Resource<CommitFiles>(() => this.requestUpdate());

  /** The commits of the pages read after the first, in the order they arrived. */
  private older: Commit[] = [];
  private loadingOlder = false;
  private olderFailure: string | null = null;
  /** What reads the page after the one read last; null at the end of the history. */
  private nextBefore: string | null = null;

  private loadedList = false;
  private loadedOid = '';
  private stopListening: (() => void) | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onStoreChange(() => {
      this.loadedList = false;
      this.loadedOid = '';
      this.requestUpdate();
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopListening?.();
    this.stopListening = null;
  }

  override updated(): void {
    if (this.mode === 'list' && !this.loadedList) {
      this.loadedList = true;
      this.older = [];
      this.olderFailure = null;
      this.nextBefore = null;
      void this.page.load(async () => {
        const first = await api.storeHistory();
        this.nextBefore = first.next_before;
        return first.commits;
      });
    }
    if (this.mode === 'commit' && this.oid !== '' && this.loadedOid !== this.oid) {
      this.loadedOid = this.oid;
      void this.commit.load(() => api.storeCommit(this.oid));
    }
  }

  /** The commits older than the last one on screen, appended to them. */
  private async loadOlder(): Promise<void> {
    const before = this.nextBefore;
    if (before === null || this.loadingOlder) return;
    this.loadingOlder = true;
    this.olderFailure = null;
    try {
      const next = await api.storeHistory(before);
      this.nextBefore = next.next_before;
      this.older = [...this.older, ...next.commits];
    } catch (caught) {
      this.olderFailure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.loadingOlder = false;
    }
  }

  private renderList(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="clock"></sl-icon><span>history</span></div>
        <h1>History</h1>
      </header>
      ${gate(
        this.page.state,
        (commits) => html`
          <fmn-history-list
            .commits=${[...commits, ...this.older]}
            .href=${(commit: Commit) => paths.historyCommit(commit.oid)}
          ></fmn-history-list>
          ${this.nextBefore === null
            ? nothing
            : html`<sl-button
                size="small"
                ?loading=${this.loadingOlder}
                @click=${() => void this.loadOlder()}
                >Load more</sl-button
              >`}
          ${this.olderFailure === null
            ? nothing
            : html`<p class="failure" role="alert">${this.olderFailure}</p>`}
        `,
      )}
    `;
  }

  /** One file of the commit: where it lives, what happened to it, and its diff. */
  private renderFile(file: CommitFile): TemplateResult {
    const target = fileTarget(file);
    return html`<section class="commit-file">
      <h2>
        ${target === null
          ? html`<code>${file.path}</code>`
          : html`<a href=${target}><code>${file.path}</code></a>`}
        <sl-badge variant="neutral" pill>${file.status}</sl-badge>
      </h2>
      <fmn-diff .diff=${file.diff}></fmn-diff>
    </section>`;
  }

  private renderCommit(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name">
          <sl-icon name="clock"></sl-icon><a href=${paths.history()}>history</a>
        </div>
      </header>
      ${gate(
        this.commit.state,
        (commit) => html`
          <fmn-history-list .commits=${[commit.commit]}></fmn-history-list>
          ${commit.files.map((file) => this.renderFile(file))}
        `,
      )}
    `;
  }

  override render(): TemplateResult {
    return this.mode === 'commit' ? this.renderCommit() : this.renderList();
  }
}

customElements.define('fmn-history-view', FmnHistoryView);
