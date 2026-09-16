import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { RequestFailed, api } from '../api/client';
import type { Commit, HistoryEntry, MemoryDoc, MemoryKind } from '../api/types';
import { suggestScopes } from '../model/tagField';
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
import { takeDeleteIntent } from '../intent';
import { paths, scopeFromSearch } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-diff';
import '../components/fmn-kind-icon';
import '../components/fmn-markdown-editor';
import '../components/fmn-side-by-side';
import '../components/fmn-tag-field';
import '../components/fmn-validation-errors';
import type { BodyChange, FmnMarkdownEditor } from '../components/fmn-markdown-editor';
import type { TagsChange } from '../components/fmn-tag-field';

export type MemoryMode = 'document' | 'history' | 'commit' | 'new';

/** A memory that is being written and has no file yet, in the scope it was started from. */
export function blankMemory(scope: string): MemoryDoc {
  return {
    id: '',
    name: '',
    title: '',
    description: '',
    kind: 'knowledge',
    scopes: scope === '' ? [] : [scope],
    source: 'user',
    metadata: {},
    created: null,
    modified: null,
    author: null,
    body: '',
    version: '',
    links: [],
    backlinks: [],
    last_commit: null,
  };
}

function missingStatus(error: Error): 'failed' | 'missing' {
  return error instanceof RequestFailed && error.status === 404 ? 'missing' : 'failed';
}

/**
 * One memory: its document and fields, edited in place, with its history. A memory
 * that does not exist yet is the same page with empty fields and an id to fill in,
 * so there is one memory view rather than two that drift apart.
 */
export class FmnMemoryPage extends PageElement {
  static override properties: PropertyDeclarations = {
    memoryId: { type: String },
    mode: { type: String },
    oid: { type: String },
    newId: { state: true },
    draft: { state: true },
    editing: { state: true },
    deleteMessage: { state: true },
    deleting: { state: true },
    deleteOpen: { state: true },
    deleteFailure: { state: true },
  };

  memoryId = '';
  mode: MemoryMode = 'document';
  oid = '';

  private readonly doc = new Resource<MemoryDoc>(() => this.requestUpdate(), {
    classify: missingStatus,
  });
  private readonly commits = new Resource<Commit[]>(() => this.requestUpdate());
  private readonly entry = new Resource<HistoryEntry>(() => this.requestUpdate());
  /** The scope ids the Scopes field offers. */
  private readonly scopeOptions = new Resource<string[]>(() => this.requestUpdate());
  private loadedOptions = false;

  /** The id of the memory being created, which is its path under memories/. */
  private newId = '';
  private draft: MemoryDraft | null = null;
  /** The field whose control is open, if any. */
  private editing: string | null = null;
  private deleteMessage = '';
  private deleting = false;
  private deleteOpen = false;
  private deleteFailure: string | null = null;

  private loadedId = '';
  private loadedHistoryId = '';
  private loadedEntry = '';
  private stopListening: (() => void) | null = null;

  private get creating(): boolean {
    return this.mode === 'new';
  }

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
    if (!this.loadedOptions) {
      this.loadedOptions = true;
      void this.scopeOptions.load(async () =>
        suggestScopes(await api.scopeIndex(), this.doc.value?.scopes ?? []),
      );
    }
    // The key a load is remembered by: a new memory is loaded once, from nothing.
    const wanted = this.creating ? 'new' : this.memoryId;
    if (wanted !== '' && this.loadedId !== wanted) {
      this.loadedId = wanted;
      this.draft = null;
      void this.doc.load(() =>
        this.creating ? Promise.resolve(blankMemory(scopeFromSearch())) : api.memory(this.memoryId),
      );
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
    if (!this.creating && this.memoryId !== '' && takeDeleteIntent('memory', this.memoryId)) {
      this.deleteOpen = true;
      void this.showDeletePanel();
    }
  }

  /** The panel the tree asked for, brought on screen with its field ready. */
  private async showDeletePanel(): Promise<void> {
    await this.updateComplete;
    const panel = this.querySelector('sl-details[open] .delete-message');
    panel?.scrollIntoView?.({ block: 'center' });
    (panel as HTMLElement | null)?.focus?.();
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
    const fields = {
      description: draft.description,
      kind: draft.kind,
      scopes: parseIdList(draft.scopesText),
      source: draft.source,
      // Straight from the editor when it has been typed in, because its own report
      // of the text arrives a moment after the keystroke that caused it.
      body: this.editor?.touched === true ? this.editor.markdown() : bodyToSave(draft),
      author: frontendAuthor,
      message: draft.message,
    };
    try {
      const outcome = this.creating
        ? await api.createMemory({ id: this.newId, ...fields })
        : await api.putMemory(this.memoryId, { ...fields, base_version: draft.baseVersion });
      if (outcome.kind === 'written') {
        announceStoreChange();
        if (this.creating) {
          navigate(paths.memory(this.newId));
          return;
        }
        this.draft = null;
        this.editing = null;
        this.loadedId = '';
        return;
      }
      if (outcome.kind === 'conflict') {
        if (this.creating) this.change({ failure: `a memory with the id ${this.newId} already exists` });
        else this.change({ conflict: outcome.conflict.current });
      } else this.change({ errors: outcome.failure.errors });
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

  private async deleteMemory(doc: MemoryDoc): Promise<void> {
    this.deleting = true;
    this.deleteFailure = null;
    try {
      const outcome = await api.deleteMemory(this.memoryId, {
        base_version: doc.version,
        author: frontendAuthor,
        message: this.deleteMessage,
      });
      if (outcome.kind === 'written') {
        this.deleteMessage = '';
        this.deleteOpen = false;
        announceStoreChange();
        navigate(paths.home());
        return;
      }
      this.deleteFailure =
        outcome.kind === 'conflict'
          ? `the memory changed on the server; its version is now ${outcome.conflict.current.version}`
          : outcome.failure.errors.map((error) => `${error.path}: ${error.message}`).join('; ');
    } catch (caught) {
      this.deleteFailure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.deleting = false;
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
      ${this.creating
        ? html`<div class="field field-id">
            <sl-input
              size="small"
              label="Id"
              help-text="The path under memories/, without .md"
              value=${this.newId}
              @sl-input=${(event: Event) => {
                this.newId = (event.target as HTMLInputElement).value;
              }}
            ></sl-input>
          </div>`
        : html`${this.renderStatic('Name', html`<span class="value-text">${doc.name}</span>`)}
            ${this.renderStatic('Id', html`<code>${doc.id}</code>`)}`}
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
        parseIdList(draft.scopesText).length === 0
          ? html`<span class="empty">none</span>`
          : html`<span class="chips"
          >${parseIdList(draft.scopesText).map(
            (scope) =>
              html`<a href=${paths.scope(scope)}
                ><sl-badge variant="neutral" pill>${scope}</sl-badge></a
              >`,
          )}</span
        >`,
        () => html`<fmn-tag-field
          label="Scopes"
          placeholder="Add a scope"
          .value=${parseIdList(draft.scopesText)}
          .suggestions=${this.scopeOptions.value ?? []}
          @fmn-tags-change=${(event: CustomEvent<TagsChange>) =>
            this.change({ scopesText: event.detail.value.join(', ') })}
        ></fmn-tag-field>`,
      )}
      ${this.renderField(
        'source',
        'Source',
        html`<span class="value-text">${draft.source}</span>`,
        () => html`<fmn-tag-field
          single
          label="Source"
          placeholder="user, assistant, or another word"
          .value=${draft.source === '' ? [] : [draft.source]}
          .suggestions=${memorySources}
          @fmn-tags-change=${(event: CustomEvent<TagsChange>) =>
            this.change({ source: event.detail.value[0] ?? '' })}
        ></fmn-tag-field>`,
      )}
      ${this.creating
        ? nothing
        : html`${this.renderStatic(
            'Modified',
            html`<span class="value-mono">${doc.modified ?? '—'}</span>`,
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
        <sl-details summary="Delete" ?open=${this.deleteOpen}>
          <p class="muted">The file is removed in a commit; the history keeps it.</p>
          <sl-input
            class="delete-message"
            size="small"
            label="Commit message"
            maxlength="72"
            value=${this.deleteMessage}
            @sl-input=${(event: Event) => {
              this.deleteMessage = (event.target as HTMLInputElement).value;
            }}
          ></sl-input>
          <sl-button
            size="small"
            variant="danger"
            ?disabled=${this.deleting || this.deleteMessage.trim() === ''}
            ?loading=${this.deleting}
            @click=${() => void this.deleteMemory(doc)}
            >Delete memory</sl-button
          >
          ${this.deleteFailure === null
            ? nothing
            : html`<p class="failure" role="alert">${this.deleteFailure}</p>`}
        </sl-details>
      </div>`}
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
    // A memory that is being created is always ready to be written: what would be
    // saved is the whole of it, including an id that is still empty.
    if (!this.creating && !isDirty(draft, doc)) return nothing;
    return html`<fmn-commit-bar
      .message=${draft.message}
      .saving=${draft.saving}
      saveLabel=${this.creating ? 'Create' : 'Save'}
      @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
        this.change({ message: event.detail.message })}
      @fmn-save=${() => void this.save()}
      @fmn-discard=${() => {
        if (this.creating) navigate(paths.home());
        else this.discard();
      }}
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
      <sl-icon slot="icon" name="exclamation-mark"></sl-icon>
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
              <span>${this.creating ? (this.newId === '' ? 'new memory' : this.newId) : doc.id}</span>
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
                  ${draft.description === ''
                    ? html`<span class="empty">The line the agent sees in an index</span>`
                    : draft.description}
                  <sl-icon-button
                    name="pencil"
                    label="Edit Description"
                    @click=${() => {
                      this.editing = 'description';
                    }}
                  ></sl-icon-button>
                </p>`}
          </header>
          ${this.creating ? nothing : this.renderTabs()}
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
