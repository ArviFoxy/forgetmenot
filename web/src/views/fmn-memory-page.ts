import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { RequestFailed, api } from '../api/client';
import type {
  Commit,
  HistoryEntry,
  MemoryDoc,
  MemoryKind,
  MemorySource,
  ValidationError,
} from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { bodyBelowTitle, frontendAuthor } from '../model/memoryBody';
import { memoryKinds, memorySources, parseIdList } from '../model/triggers';
import { announceStoreChange, navigate, onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-diff';
import '../components/fmn-kind-icon';
import '../components/fmn-markdown';
import '../components/fmn-side-by-side';
import '../components/fmn-source-editor';
import '../components/fmn-validation-errors';

export type MemoryMode = 'document' | 'edit' | 'history' | 'commit';

/** The edit in progress, against the version it was started from. */
interface Draft {
  baseVersion: string;
  description: string;
  kind: MemoryKind;
  scopesText: string;
  source: MemorySource;
  body: string;
  message: string;
  preview: boolean;
  saving: boolean;
  errors: ValidationError[];
  conflict: MemoryDoc | null;
  failure: string | null;
}

function draftOf(doc: MemoryDoc): Draft {
  return {
    baseVersion: doc.version,
    description: doc.description,
    kind: doc.kind,
    scopesText: doc.scopes.join(', '),
    source: doc.source,
    body: doc.body,
    message: '',
    preview: true,
    saving: false,
    errors: [],
    conflict: null,
    failure: null,
  };
}

function missingStatus(error: Error): 'failed' | 'missing' {
  return error instanceof RequestFailed && error.status === 404 ? 'missing' : 'failed';
}

/** One memory: its document, its fields, its history and one commit of it. */
export class FmnMemoryPage extends PageElement {
  static override properties: PropertyDeclarations = {
    memoryId: { type: String },
    mode: { type: String },
    oid: { type: String },
    draft: { state: true },
    archiveMessage: { state: true },
    archiving: { state: true },
    archiveFailure: { state: true },
  };

  memoryId = '';
  mode: MemoryMode = 'document';
  oid = '';

  private readonly doc = new Resource<MemoryDoc>(() => this.requestUpdate(), { classify: missingStatus });
  private readonly commits = new Resource<Commit[]>(() => this.requestUpdate());
  private readonly entry = new Resource<HistoryEntry>(() => this.requestUpdate());

  private draft: Draft | null = null;
  private archiveMessage = '';
  private archiving = false;
  private archiveFailure: string | null = null;

  private loadedId = '';
  private loadedHistoryId = '';
  private loadedEntry = '';
  private stopListening: (() => void) | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onStoreChange(() => {
      this.loadedId = '';
      this.loadedHistoryId = '';
      this.loadedEntry = '';
      this.requestUpdate();
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopListening?.();
    this.stopListening = null;
  }

  // Loads and the edit draft are set up after a render, never during one, so an
  // update never changes the state the same update is rendering.
  override updated(): void {
    this.syncLoads();
    this.syncDraft();
  }

  private syncLoads(): void {
    if (this.memoryId !== '' && this.loadedId !== this.memoryId) {
      this.loadedId = this.memoryId;
      void this.doc.load(() => api.memory(this.memoryId));
    }
    const wantsHistory = this.mode === 'history' || this.mode === 'commit';
    if (wantsHistory && this.loadedHistoryId !== this.memoryId) {
      this.loadedHistoryId = this.memoryId;
      void this.commits.load(() => api.memoryHistory(this.memoryId));
    }
    const entryKey = `${this.memoryId}@${this.oid}`;
    if (this.mode === 'commit' && this.oid !== '' && this.loadedEntry !== entryKey) {
      this.loadedEntry = entryKey;
      void this.entry.load(() => api.memoryHistoryEntry(this.memoryId, this.oid));
    }
  }

  /** The draft is for one loaded version; a new version or leaving edit mode drops it. */
  private syncDraft(): void {
    const doc = this.doc.value;
    if (this.mode === 'edit' && doc !== null) {
      if (this.draft === null || this.draft.baseVersion !== doc.version) this.draft = draftOf(doc);
      return;
    }
    if (this.mode !== 'edit' && this.draft !== null) this.draft = null;
  }

  private change(change: Partial<Draft>): void {
    if (this.draft === null) return;
    this.draft = { ...this.draft, ...change };
  }

  private async save(): Promise<void> {
    const draft = this.draft;
    if (draft === null) return;
    this.change({ saving: true, errors: [], conflict: null, failure: null });
    try {
      const outcome = await api.putMemory(this.memoryId, {
        description: draft.description,
        kind: draft.kind,
        scopes: parseIdList(draft.scopesText),
        source: draft.source,
        body: draft.body,
        base_version: draft.baseVersion,
        author: frontendAuthor,
        message: draft.message,
      });
      if (outcome.kind === 'written') {
        this.draft = null;
        announceStoreChange();
        navigate(paths.memory(this.memoryId));
        return;
      }
      if (outcome.kind === 'conflict') this.change({ conflict: outcome.conflict.current });
      else this.change({ errors: outcome.failure.errors });
    } catch (caught) {
      this.change({ failure: caught instanceof Error ? caught.message : String(caught) });
    } finally {
      this.change({ saving: false });
      void this.showWriteOutcome();
    }
  }

  /** A refused write is reported below the editor, which may be off screen. */
  private async showWriteOutcome(): Promise<void> {
    await this.updateComplete;
    const reported = this.querySelector('.conflict, fmn-validation-errors table, p.failure');
    reported?.scrollIntoView?.({ block: 'center' });
  }

  private async archive(doc: MemoryDoc): Promise<void> {
    this.archiving = true;
    this.archiveFailure = null;
    try {
      const outcome = await api.archiveMemory(this.memoryId, {
        base_version: doc.version,
        author: frontendAuthor,
        message: this.archiveMessage,
      });
      if (outcome.kind === 'written') {
        this.archiveMessage = '';
        announceStoreChange();
        return;
      }
      this.archiveFailure =
        outcome.kind === 'conflict'
          ? `the memory changed on the server; its version is now ${outcome.conflict.current.version}`
          : outcome.failure.errors.map((error) => `${error.path}: ${error.message}`).join('; ');
    } catch (caught) {
      this.archiveFailure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.archiving = false;
    }
  }

  private renderTabs(): TemplateResult {
    const tabs: { href: string; label: string; active: boolean }[] = [
      { href: paths.memory(this.memoryId), label: 'Document', active: this.mode === 'document' },
      { href: paths.memoryEdit(this.memoryId), label: 'Edit', active: this.mode === 'edit' },
      {
        href: paths.memoryHistory(this.memoryId),
        label: 'History',
        active: this.mode === 'history' || this.mode === 'commit',
      },
    ];
    return html`<nav class="tabs" aria-label="Memory views">
      <ul>
        ${tabs.map(
          (tab) => html`<li>
            <a href=${tab.href} aria-current=${tab.active ? 'page' : 'false'}>${tab.label}</a>
          </li>`,
        )}
      </ul>
    </nav>`;
  }

  private renderInfobox(doc: MemoryDoc): TemplateResult {
    return html`<aside class="infobox">
      <dl>
        <dt>Name</dt>
        <dd>${doc.name}</dd>
        <dt>Id</dt>
        <dd><code>${doc.id}</code></dd>
        <dt>Kind</dt>
        <dd><fmn-kind-icon kind=${doc.kind}></fmn-kind-icon> ${doc.kind}</dd>
        <dt>Scopes</dt>
        <dd>
          ${doc.scopes.map((scope) => html`<a class="chip" href=${paths.scope(scope)}>${scope}</a>`)}
        </dd>
        <dt>Source</dt>
        <dd>${doc.source}</dd>
        <dt>Modified</dt>
        <dd>${doc.modified ?? ''}</dd>
        <dt>Archived</dt>
        <dd>${doc.archived ? 'yes' : 'no'}</dd>
        <dt>Version</dt>
        <dd><code>${doc.version}</code></dd>
        <dt>Backlinks</dt>
        <dd>
          ${doc.backlinks.length === 0
            ? html`<span class="empty">none</span>`
            : doc.backlinks.map((name) => html`<a class="chip" href=${paths.memory(name)}>${name}</a>`)}
        </dd>
      </dl>
    </aside>`;
  }

  private renderDocument(doc: MemoryDoc): TemplateResult {
    return html`
      <div class="memory-layout">
        <div class="document">
          <fmn-markdown .text=${bodyBelowTitle(doc.body, doc.title)}></fmn-markdown>
          <details class="archive-panel">
            <summary role="button" class="secondary outline">Archive</summary>
            <fmn-commit-bar
              fieldId="archive-message"
              saveLabel="Archive memory"
              .message=${this.archiveMessage}
              .saving=${this.archiving}
              @fmn-message-change=${(event: CustomEvent<{ message: string }>) => {
                this.archiveMessage = event.detail.message;
              }}
              @fmn-save=${() => void this.archive(doc)}
            ></fmn-commit-bar>
            ${this.archiveFailure === null
              ? nothing
              : html`<p class="failure" role="alert">${this.archiveFailure}</p>`}
          </details>
        </div>
        ${this.renderInfobox(doc)}
      </div>
    `;
  }

  private renderEdit(doc: MemoryDoc): TemplateResult {
    const draft = this.draft;
    if (draft === null) return html`<p aria-busy="true">Loading</p>`;
    return html`
      <div class="field-grid">
        <label for="memory-description">Description</label>
        <input
          id="memory-description"
          type="text"
          required
          .value=${draft.description}
          @input=${(event: Event) =>
            this.change({ description: (event.target as HTMLInputElement).value })}
        />

        <label for="memory-kind">Kind</label>
        <select
          id="memory-kind"
          @change=${(event: Event) =>
            this.change({ kind: (event.target as HTMLSelectElement).value as MemoryKind })}
        >
          ${memoryKinds.map(
            (kind) => html`<option value=${kind} ?selected=${kind === draft.kind}>${kind}</option>`,
          )}
        </select>

        <label for="memory-scopes">Scopes</label>
        <input
          id="memory-scopes"
          type="text"
          .value=${draft.scopesText}
          @input=${(event: Event) =>
            this.change({ scopesText: (event.target as HTMLInputElement).value })}
        />

        <label for="memory-source">Source</label>
        <select
          id="memory-source"
          @change=${(event: Event) =>
            this.change({ source: (event.target as HTMLSelectElement).value as MemorySource })}
        >
          ${memorySources.map(
            (source) =>
              html`<option value=${source} ?selected=${source === draft.source}>${source}</option>`,
          )}
        </select>
      </div>

      <label class="preview-toggle">
        <input
          type="checkbox"
          ?checked=${draft.preview}
          @change=${(event: Event) =>
            this.change({ preview: (event.target as HTMLInputElement).checked })}
        />
        Preview
      </label>

      <div class="edit-split" ?data-preview=${draft.preview}>
        <fmn-source-editor
          label="Body"
          resetKey=${draft.baseVersion}
          .value=${draft.body}
          @fmn-source-change=${(event: CustomEvent<{ value: string }>) =>
            this.change({ body: event.detail.value })}
        ></fmn-source-editor>
        ${draft.preview
          ? html`<div class="document preview">
              <p class="field-label">Rendered</p>
              <h1>${doc.title}</h1>
              <fmn-markdown .text=${bodyBelowTitle(draft.body, doc.title)}></fmn-markdown>
            </div>`
          : nothing}
      </div>

      <fmn-commit-bar
        fieldId="commit-message"
        saveLabel="Save"
        .message=${draft.message}
        .saving=${draft.saving}
        cancelHref=${paths.memory(this.memoryId)}
        @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
          this.change({ message: event.detail.message })}
        @fmn-save=${() => void this.save()}
      ></fmn-commit-bar>

      <fmn-validation-errors .errors=${draft.errors}></fmn-validation-errors>
      ${draft.failure === null ? nothing : html`<p class="failure" role="alert">${draft.failure}</p>`}
      ${draft.conflict === null ? nothing : this.renderConflict(draft, draft.conflict)}
    `;
  }

  private renderConflict(draft: Draft, current: MemoryDoc): TemplateResult {
    return html`<section class="conflict" role="alert">
      <h2>Conflict</h2>
      <dl class="inline-fields">
        <dt>Loaded version</dt>
        <dd><code>${draft.baseVersion}</code></dd>
        <dt>Current version</dt>
        <dd><code>${current.version}</code></dd>
      </dl>
      <fmn-side-by-side
        leftLabel="Your text"
        .leftText=${draft.body}
        rightLabel="Current on server"
        .rightText=${current.body}
      ></fmn-side-by-side>
      <button
        type="button"
        @click=${() => {
          this.draft = null;
          this.loadedId = '';
          this.requestUpdate();
        }}
      >
        Reload current version
      </button>
    </section>`;
  }

  private renderHistory(): TemplateResult {
    return gate(
      this.commits.state,
      (commits) => html`<figure>
        <table>
          <thead>
            <tr>
              <th scope="col">Title</th>
              <th scope="col">Time</th>
              <th scope="col">Author</th>
              <th scope="col">Commit</th>
            </tr>
          </thead>
          <tbody>
            ${commits.map(
              (commit) => html`<tr>
                <td><a href=${paths.memoryCommit(this.memoryId, commit.oid)}>${commit.title}</a></td>
                <td>${commit.time}</td>
                <td>${commit.author}</td>
                <td><code>${commit.oid.slice(0, 10)}</code></td>
              </tr>`,
            )}
          </tbody>
        </table>
      </figure>`,
    );
  }

  private renderCommit(): TemplateResult {
    return html`
      ${gate(
        this.entry.state,
        (entry) => html`
          <dl class="inline-fields">
            <dt>Title</dt>
            <dd>${entry.commit.title}</dd>
            <dt>Time</dt>
            <dd>${entry.commit.time}</dd>
            <dt>Author</dt>
            <dd>${entry.commit.author}</dd>
            <dt>Commit</dt>
            <dd><code>${entry.commit.oid}</code></dd>
          </dl>
          <h2>Diff</h2>
          <fmn-diff .diff=${entry.diff}></fmn-diff>
          <h2>Content</h2>
          <pre class="content">${entry.content}</pre>
        `,
      )}
    `;
  }

  private renderMode(doc: MemoryDoc): TemplateResult {
    switch (this.mode) {
      case 'edit':
        return this.renderEdit(doc);
      case 'history':
        return this.renderHistory();
      case 'commit':
        return this.renderCommit();
      default:
        return this.renderDocument(doc);
    }
  }

  override render(): TemplateResult {
    return gate(
      this.doc.state,
      (doc) => html`
        <header class="page-header">
          <h1>${doc.title}</h1>
          <p class="description">${doc.description}</p>
          ${doc.archived ? html`<p class="badge-archived">archived</p>` : nothing}
          ${this.renderTabs()}
        </header>
        ${this.renderMode(doc)}
      `,
      () => html`<p class="failure" role="alert">No memory with the id ${this.memoryId}</p>`,
    );
  }
}

customElements.define('fmn-memory-page', FmnMemoryPage);
