import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { RequestFailed, api } from '../api/client';
import type { Commit, HistoryEntry, MemoryDoc, MemoryKind, MemorySource } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { frontendAuthor } from '../model/author';
import {
  bodyToSave,
  draftOf,
  isDirty,
  memoryText,
  type MemoryDraft,
} from '../model/memoryDraft';
import { memoryKinds, memorySources, parseIdList } from '../model/triggers';
import { announceStoreChange, navigate, onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-diff';
import '../components/fmn-kind-icon';
import '../components/fmn-markdown-editor';
import '../components/fmn-side-by-side';
import '../components/fmn-validation-errors';
import type { BodyChange, FmnMarkdownEditor } from '../components/fmn-markdown-editor';

export type MemoryMode = 'document' | 'history' | 'commit';

function missingStatus(error: Error): 'failed' | 'missing' {
  return error instanceof RequestFailed && error.status === 404 ? 'missing' : 'failed';
}

/** One memory: its document and fields, edited in place, with its history. */
export class FmnMemoryPage extends PageElement {
  static override properties: PropertyDeclarations = {
    memoryId: { type: String },
    mode: { type: String },
    oid: { type: String },
    draft: { state: true },
    editing: { state: true },
    archiveMessage: { state: true },
    archiving: { state: true },
    archiveFailure: { state: true },
  };

  memoryId = '';
  mode: MemoryMode = 'document';
  oid = '';

  private readonly doc = new Resource<MemoryDoc>(() => this.requestUpdate(), {
    classify: missingStatus,
  });
  private readonly commits = new Resource<Commit[]>(() => this.requestUpdate());
  private readonly entry = new Resource<HistoryEntry>(() => this.requestUpdate());

  private draft: MemoryDraft | null = null;
  /** The field whose control is open, if any. */
  private editing: string | null = null;
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

  override updated(): void {
    if (this.memoryId !== '' && this.loadedId !== this.memoryId) {
      this.loadedId = this.memoryId;
      this.draft = null;
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
    const doc = this.doc.value;
    if (doc !== null && (this.draft === null || this.draft.baseVersion !== doc.version)) {
      this.draft = draftOf(doc);
    }
  }

  private change(change: Partial<MemoryDraft>): void {
    if (this.draft === null) return;
    this.draft = { ...this.draft, ...change };
  }

  private discard(): void {
    const doc = this.doc.value;
    if (doc === null) return;
    this.draft = draftOf(doc);
    this.editing = null;
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
        body: bodyToSave(draft),
        base_version: draft.baseVersion,
        author: frontendAuthor,
        message: draft.message,
      });
      if (outcome.kind === 'written') {
        this.draft = null;
        this.editing = null;
        this.loadedId = '';
        announceStoreChange();
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

  /** A refused write is reported below the document, which may be off screen. */
  private async showWriteOutcome(): Promise<void> {
    await this.updateComplete;
    const reported = this.querySelector('.conflict, fmn-validation-errors sl-alert, p.failure');
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
        this.loadedId = '';
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
    const active = this.mode === 'document' ? 'document' : 'history';
    return html`<sl-tab-group
      class="page-tabs"
      @sl-tab-show=${(event: CustomEvent<{ name: string }>) => {
        const wanted = event.detail.name === 'document'
          ? paths.memory(this.memoryId)
          : paths.memoryHistory(this.memoryId);
        navigate(wanted);
      }}
    >
      <sl-tab slot="nav" panel="document" ?active=${active === 'document'}>
        <sl-icon name="file-text"></sl-icon>Document
      </sl-tab>
      <sl-tab slot="nav" panel="history" ?active=${active === 'history'}>
        <sl-icon name="clock"></sl-icon>History
      </sl-tab>
    </sl-tab-group>`;
  }

  /** A field that shows its value and opens a control when asked. */
  private renderField(
    name: string,
    label: string,
    value: TemplateResult,
    control: () => TemplateResult,
  ): TemplateResult {
    return html`<div class="field">
      <span class="field-label">${label}</span>
      ${this.editing === name
        ? html`<div class="inline-edit">
            ${control()}
            <sl-icon-button
              name="x"
              label="Close ${label}"
              @click=${() => {
                this.editing = null;
              }}
            ></sl-icon-button>
          </div>`
        : html`<div class="field-value">
            ${value}
            <sl-icon-button
              name="pencil"
              label="Edit ${label}"
              @click=${() => {
                this.editing = name;
              }}
            ></sl-icon-button>
          </div>`}
    </div>`;
  }

  private renderStatic(label: string, value: TemplateResult): TemplateResult {
    return html`<div class="field">
      <span class="field-label">${label}</span>
      <div class="field-value">${value}</div>
    </div>`;
  }

  private renderInfobox(doc: MemoryDoc, draft: MemoryDraft): TemplateResult {
    return html`<aside class="infobox">
      ${this.renderStatic('Name', html`<span class="value-text">${doc.name}</span>`)}
      ${this.renderStatic('Id', html`<code>${doc.id}</code>`)}
      ${this.renderField(
        'kind',
        'Kind',
        html`<fmn-kind-icon kind=${draft.kind}></fmn-kind-icon
        ><span class="value-text">${draft.kind}</span>`,
        () => html`<sl-select
          size="small"
          value=${draft.kind}
          hoist
          @sl-change=${(event: Event) =>
            this.change({ kind: (event.target as HTMLInputElement).value as MemoryKind })}
        >
          ${memoryKinds.map((kind) => html`<sl-option value=${kind}>${kind}</sl-option>`)}
        </sl-select>`,
      )}
      ${this.renderField(
        'scopes',
        'Scopes',
        html`<span class="chips"
          >${parseIdList(draft.scopesText).map(
            (scope) =>
              html`<a href=${paths.scope(scope)}
                ><sl-badge variant="neutral" pill>${scope}</sl-badge></a
              >`,
          )}</span
        >`,
        () => html`<sl-input
          size="small"
          value=${draft.scopesText}
          help-text="Comma separated"
          @sl-input=${(event: Event) =>
            this.change({ scopesText: (event.target as HTMLInputElement).value })}
        ></sl-input>`,
      )}
      ${this.renderField(
        'source',
        'Source',
        html`<span class="value-text">${draft.source}</span>`,
        () => html`<sl-select
          size="small"
          value=${draft.source}
          hoist
          @sl-change=${(event: Event) =>
            this.change({ source: (event.target as HTMLInputElement).value as MemorySource })}
        >
          ${memorySources.map((source) => html`<sl-option value=${source}>${source}</sl-option>`)}
        </sl-select>`,
      )}
      ${this.renderStatic('Modified', html`<span class="value-mono">${doc.modified ?? '—'}</span>`)}
      ${this.renderStatic(
        'Archived',
        html`<span class="value-text">${doc.archived ? 'yes' : 'no'}</span>`,
      )}
      ${this.renderStatic('Version', html`<code>${doc.version}</code>`)}
      ${this.renderStatic(
        'Links',
        doc.links.length === 0
          ? html`<span class="empty">none</span>`
          : html`<span class="chips"
              >${doc.links.map(
                (name) =>
                  html`<a href=${paths.memory(name)}
                    ><sl-badge variant="neutral" pill>${name}</sl-badge></a
                  >`,
              )}</span
            >`,
      )}
      ${this.renderStatic(
        'Backlinks',
        doc.backlinks.length === 0
          ? html`<span class="empty">none</span>`
          : html`<span class="chips"
              >${doc.backlinks.map(
                (name) =>
                  html`<a href=${paths.memory(name)}
                    ><sl-badge variant="neutral" pill>${name}</sl-badge></a
                  >`,
              )}</span
            >`,
      )}
      <div class="field">
        <sl-details summary="Archive">
          <sl-input
            size="small"
            label="Commit message"
            maxlength="72"
            value=${this.archiveMessage}
            @sl-input=${(event: Event) => {
              this.archiveMessage = (event.target as HTMLInputElement).value;
            }}
          ></sl-input>
          <sl-button
            size="small"
            variant="default"
            ?disabled=${this.archiving || this.archiveMessage.trim() === ''}
            ?loading=${this.archiving}
            @click=${() => void this.archive(doc)}
            >Archive memory</sl-button
          >
          ${this.archiveFailure === null
            ? nothing
            : html`<p class="failure" role="alert">${this.archiveFailure}</p>`}
        </sl-details>
      </div>
    </aside>`;
  }

  private renderDocument(doc: MemoryDoc, draft: MemoryDraft): TemplateResult {
    return html`
      <div class="memory-layout">
        <div class="document">
          <fmn-markdown-editor
            resetKey=${draft.baseVersion}
            .value=${draft.originalBody}
            @fmn-body-change=${(event: CustomEvent<BodyChange>) =>
              this.change({ editedBody: event.detail.value })}
          ></fmn-markdown-editor>
        </div>
        ${this.renderInfobox(doc, draft)}
      </div>
      ${this.renderCommitBar(doc, draft)} ${this.renderOutcome(draft)}
    `;
  }

  private renderCommitBar(doc: MemoryDoc, draft: MemoryDraft): TemplateResult | typeof nothing {
    if (!isDirty(draft, doc)) return nothing;
    return html`<fmn-commit-bar
      .message=${draft.message}
      .saving=${draft.saving}
      saveLabel="Save"
      @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
        this.change({ message: event.detail.message })}
      @fmn-save=${() => void this.save()}
      @fmn-discard=${() => this.discard()}
    ></fmn-commit-bar>`;
  }

  private renderOutcome(draft: MemoryDraft): TemplateResult {
    return html`
      <fmn-validation-errors .errors=${draft.errors}></fmn-validation-errors>
      ${draft.failure === null ? nothing : html`<p class="failure" role="alert">${draft.failure}</p>`}
      ${draft.conflict === null ? nothing : this.renderConflict(draft, draft.conflict)}
    `;
  }

  private renderConflict(draft: MemoryDraft, current: MemoryDoc): TemplateResult {
    const mine = memoryText({
      description: draft.description,
      kind: draft.kind,
      scopes: parseIdList(draft.scopesText),
      source: draft.source,
      body: bodyToSave(draft),
    });
    const theirs = memoryText({
      description: current.description,
      kind: current.kind,
      scopes: current.scopes,
      source: current.source,
      body: current.body,
    });
    // The alert is wrapped, because a Shoelace alert host is display: contents and
    // so has no box of its own to place or scroll to.
    return html`<section class="conflict">
      <sl-alert variant="warning" open>
      <sl-icon slot="icon" name="circle-alert"></sl-icon>
      <strong>Conflict</strong>
      <div class="conflict-versions">
        <span>Loaded version <code>${draft.baseVersion}</code></span>
        <span>Current version <code>${current.version}</code></span>
      </div>
      <fmn-side-by-side
        leftLabel="Your text"
        .leftText=${mine}
        rightLabel="Current on server"
        .rightText=${theirs}
      ></fmn-side-by-side>
        <sl-button
          size="small"
          @click=${() => {
            this.draft = null;
            this.loadedId = '';
            this.requestUpdate();
          }}
          >Reload current version</sl-button
        >
      </sl-alert>
    </section>`;
  }

  private renderHistory(): TemplateResult {
    return gate(
      this.commits.state,
      (commits) => html`<div class="table-wrap">
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
            ${commits.map(
              (commit) => html`<tr>
                <td><a href=${paths.memoryCommit(this.memoryId, commit.oid)}>${commit.title}</a></td>
                <td class="nowrap">${commit.time}</td>
                <td>${commit.author}</td>
                <td><code>${commit.oid.slice(0, 10)}</code></td>
              </tr>`,
            )}
          </tbody>
        </table>
      </div>`,
    );
  }

  private renderCommit(): TemplateResult {
    return gate(
      this.entry.state,
      (entry) => html`
        <div class="infobox">
          ${this.renderStatic('Title', html`<span class="value-text">${entry.commit.title}</span>`)}
          ${this.renderStatic('Time', html`<span class="value-mono">${entry.commit.time}</span>`)}
          ${this.renderStatic('Author', html`<span class="value-text">${entry.commit.author}</span>`)}
          ${this.renderStatic('Commit', html`<code>${entry.commit.oid}</code>`)}
        </div>
        <h2>Diff</h2>
        <fmn-diff .diff=${entry.diff}></fmn-diff>
        <h2>Content</h2>
        <pre class="content">${entry.content}</pre>
      `,
    );
  }

  override render(): TemplateResult {
    return gate(
      this.doc.state,
      (doc) => {
        const draft = this.draft ?? draftOf(doc);
        return html`
          <header class="page-header">
            <div class="page-name">
              <fmn-kind-icon kind=${draft.kind}></fmn-kind-icon>
              <span>${doc.id}</span>
              ${doc.archived ? html`<sl-badge variant="neutral">archived</sl-badge>` : nothing}
            </div>
            ${this.editing === 'description'
              ? html`<div class="inline-edit">
                  <sl-input
                    size="small"
                    label="Description"
                    value=${draft.description}
                    @sl-input=${(event: Event) =>
                      this.change({ description: (event.target as HTMLInputElement).value })}
                  ></sl-input>
                  <sl-icon-button
                    name="x"
                    label="Close Description"
                    @click=${() => {
                      this.editing = null;
                    }}
                  ></sl-icon-button>
                </div>`
              : html`<p class="description">
                  ${draft.description}
                  <sl-icon-button
                    name="pencil"
                    label="Edit Description"
                    @click=${() => {
                      this.editing = 'description';
                    }}
                  ></sl-icon-button>
                </p>`}
          </header>
          ${this.renderTabs()}
          ${this.mode === 'history'
            ? this.renderHistory()
            : this.mode === 'commit'
              ? this.renderCommit()
              : this.renderDocument(doc, draft)}
        `;
      },
      () => html`<p class="failure" role="alert">No memory with the id ${this.memoryId}</p>`,
    );
  }

  /** The editor element, for tests and for reading the markdown on demand. */
  get editor(): FmnMarkdownEditor | null {
    return this.querySelector('fmn-markdown-editor');
  }
}

customElements.define('fmn-memory-page', FmnMemoryPage);
