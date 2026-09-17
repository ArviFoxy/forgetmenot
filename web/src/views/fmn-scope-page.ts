import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { MemorySummary, ScopeDoc, ScopeRow, Trigger, ValidationError } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { frontendAuthor } from '../model/author';
import { fieldOf, parseIdList } from '../model/triggers';
import { announceStoreChange, navigate, onStoreChange } from '../navigation';
import { takeDeleteIntent } from '../intent';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-kind-icon';
import '../components/fmn-side-by-side';
import '../components/fmn-tag-field';
import '../components/fmn-trigger-table';
import '../components/fmn-trigger-test';
import '../components/fmn-validation-errors';
import type { TagsChange } from '../components/fmn-tag-field';

/** A scope that is being written and has no file yet. */
export function blankScope(): ScopeDoc {
  return { id: '', message: null, implies: [], triggers: [], version: '' };
}

/** The row of the scope being created: a file scope whose file is still empty. */
function blankRow(): ScopeRow {
  return { id: '', kind: 'file', name: null, file: blankScope() };
}

interface ScopeDraft {
  baseVersion: string;
  impliesText: string;
  triggers: Trigger[];
  /** The scope's message; empty means the scope carries none. */
  scopeMessage: string;
  /** The commit message of the write. */
  message: string;
  saving: boolean;
  errors: ValidationError[];
  conflict: ScopeDoc | null;
  failure: string | null;
}

function draftOf(scope: ScopeDoc): ScopeDraft {
  return {
    baseVersion: scope.version,
    impliesText: scope.implies.join(', '),
    triggers: scope.triggers,
    scopeMessage: scope.message ?? '',
    message: '',
    saving: false,
    errors: [],
    conflict: null,
    failure: null,
  };
}

/** The message a write sends: what the field holds, or null when it is blank. */
function messageToSave(draft: { scopeMessage: string }): string | null {
  return draft.scopeMessage.trim() === '' ? null : draft.scopeMessage;
}

/** The scope fields as one text, so the two sides of a conflict can be compared. */
function scopeText(scope: { message: string | null; implies: string[]; triggers: Trigger[] }): string {
  const lines = [`message: ${scope.message ?? ''}`, `implies: ${scope.implies.join(', ')}`];
  for (const trigger of scope.triggers) {
    lines.push(
      `trigger: ${fieldOf(trigger)} ${trigger.pattern}${trigger.machine ? ` @${trigger.machine}` : ''}`,
    );
  }
  return lines.join('\n');
}

function triggersEqual(left: Trigger[], right: Trigger[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export type ScopeMode = 'scope' | 'new';

/**
 * One scope: what it implies, its triggers and its memories, edited in place. A
 * scope that does not exist yet is the same page with empty fields and an id to
 * fill in, so there is one scope view rather than two that drift apart.
 */
export class FmnScopePage extends PageElement {
  static override properties: PropertyDeclarations = {
    scopeId: { type: String },
    mode: { type: String },
    newId: { state: true },
    draft: { state: true },
    editing: { state: true },
    deleteMessage: { state: true },
    deleting: { state: true },
    deleteOpen: { state: true },
    deleteErrors: { state: true },
    deleteFailure: { state: true },
  };

  scopeId = '';
  mode: ScopeMode = 'scope';

  /** The scope index row of this id, or null when no scope has it. */
  private readonly row = new Resource<ScopeRow | null>(() => this.requestUpdate());
  private readonly memories = new Resource<MemorySummary[]>(() => this.requestUpdate());
  /** The scope ids the Implies field offers: the other scopes with a file. */
  private readonly scopeOptions = new Resource<string[]>(() => this.requestUpdate());
  /** The machine names the trigger table and the trigger test offer. */
  private readonly machines = new Resource<string[]>(() => this.requestUpdate());
  private loadedOptions = false;

  /** The id of the scope being created, which is the name of its file. */
  private newId = '';
  private draft: ScopeDraft | null = null;
  private editing: string | null = null;
  private deleteMessage = '';
  private deleting = false;
  private deleteOpen = false;
  private deleteErrors: ValidationError[] = [];
  private deleteFailure: string | null = null;

  private loadedId = '';
  private stopListening: (() => void) | null = null;

  private get creating(): boolean {
    return this.mode === 'new';
  }

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onStoreChange(() => {
      this.loadedId = '';
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
        (await api.scopeIndex())
          .filter((row) => row.file !== null && row.id !== this.scopeId)
          .map((row) => row.id),
      );
      void this.machines.load(() => api.machineIndex());
    }
    // The key a load is remembered by: a new scope is loaded once, from nothing.
    const wanted = this.creating ? 'new' : this.scopeId;
    if (wanted !== '' && this.loadedId !== wanted) {
      this.loadedId = wanted;
      this.draft = null;
      if (this.creating) {
        void this.row.load(() => Promise.resolve(blankRow()));
        this.memories.reset();
      } else {
        void this.row.load(async () => {
          const rows = await api.scopeIndex();
          return rows.find((row) => row.id === this.scopeId) ?? null;
        });
        void this.memories.load(() => api.memoryIndex({ scope: this.scopeId }));
      }
    }
    const file = this.row.value?.file ?? null;
    if (file !== null && (this.draft === null || this.draft.baseVersion !== file.version)) {
      this.draft = draftOf(file);
    }
    if (!this.creating && this.scopeId !== '' && takeDeleteIntent('scope', this.scopeId)) {
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

  /**
   * Deletes the scope's file. The server refuses while a memory still lists the
   * scope or another scope implies it, and names those files; they are shown so the
   * reader knows what to change first.
   */
  private async deleteScope(scope: ScopeDoc): Promise<void> {
    this.deleting = true;
    this.deleteErrors = [];
    this.deleteFailure = null;
    try {
      const outcome = await api.deleteScope(this.scopeId, {
        base_version: scope.version,
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
      if (outcome.kind === 'invalid') this.deleteErrors = outcome.failure.errors;
      else {
        this.deleteFailure = `the scope changed on the server; its version is now ${outcome.conflict.current.version}`;
      }
    } catch (caught) {
      this.deleteFailure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.deleting = false;
    }
  }

  private change(change: Partial<ScopeDraft>): void {
    if (this.draft === null) return;
    this.draft = { ...this.draft, ...change };
  }

  private dirty(scope: ScopeDoc, draft: ScopeDraft): boolean {
    // A scope that is being created is always ready to be written: what would be
    // saved is the whole of it, including an id that is still empty.
    if (this.creating) return true;
    if (parseIdList(draft.impliesText).join(',') !== scope.implies.join(',')) return true;
    if (messageToSave(draft) !== scope.message) return true;
    return !triggersEqual(draft.triggers, scope.triggers);
  }

  private async save(): Promise<void> {
    const draft = this.draft;
    if (draft === null) return;
    this.change({ saving: true, errors: [], conflict: null, failure: null });
    try {
      const outcome = this.creating
        ? await api.createScope({
            id: this.newId,
            implies: parseIdList(draft.impliesText),
            triggers: draft.triggers,
            scope_message: messageToSave(draft),
            author: frontendAuthor,
            message: draft.message,
          })
        : await api.putScope(this.scopeId, {
            implies: parseIdList(draft.impliesText),
            triggers: draft.triggers,
            scope_message: messageToSave(draft),
            base_version: draft.baseVersion,
            author: frontendAuthor,
            message: draft.message,
          });
      if (outcome.kind === 'written') {
        announceStoreChange();
        if (this.creating) {
          navigate(paths.scope(this.newId));
          return;
        }
        this.draft = null;
        this.editing = null;
        this.loadedId = '';
        return;
      }
      if (outcome.kind === 'conflict') {
        if (this.creating) this.change({ failure: `a scope with the id ${this.newId} already exists` });
        else this.change({ conflict: outcome.conflict.current });
      } else this.change({ errors: outcome.failure.errors });
    } catch (caught) {
      this.change({ failure: caught instanceof Error ? caught.message : String(caught) });
    } finally {
      this.change({ saving: false });
      void this.showWriteOutcome();
    }
  }

  private async showWriteOutcome(): Promise<void> {
    await this.updateComplete;
    const reported = this.querySelector('.conflict, fmn-validation-errors sl-alert, p.failure');
    reported?.scrollIntoView?.({ block: 'center' });
  }

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

  private renderMemories(): TemplateResult {
    return gate(this.memories.state, (memories) =>
      memories.length === 0
        ? html`<p class="empty">No memories in this scope</p>`
        : html`<ul class="memory-index">
            ${memories.map(
              (memory) => html`<li>
                <fmn-kind-icon kind=${memory.kind}></fmn-kind-icon>
                <a href=${paths.memory(memory.id)}>${memory.name}</a>
                <span class="muted">${memory.description}</span>
              </li>`,
            )}
          </ul>`,
    );
  }

  private renderTriggers(scope: ScopeDoc, draft: ScopeDraft): TemplateResult {
    return html`
      <h2>Triggers</h2>
      <fmn-trigger-table
        .triggers=${draft.triggers}
        .machines=${this.machines.value ?? []}
        @fmn-triggers-change=${(event: CustomEvent<{ triggers: Trigger[] }>) =>
          this.change({ triggers: event.detail.triggers })}
      ></fmn-trigger-table>
      ${this.dirty(scope, draft)
        ? html`<fmn-commit-bar
            .message=${draft.message}
            .saving=${draft.saving}
            saveLabel=${this.creating ? 'Create' : 'Save'}
            @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
              this.change({ message: event.detail.message })}
            @fmn-save=${() => void this.save()}
            @fmn-discard=${() => {
              if (this.creating) {
                navigate(paths.home());
                return;
              }
              this.draft = draftOf(scope);
              this.editing = null;
            }}
          ></fmn-commit-bar>`
        : nothing}
      <fmn-validation-errors .errors=${draft.errors}></fmn-validation-errors>
      ${draft.failure === null ? nothing : html`<p class="failure" role="alert">${draft.failure}</p>`}
      ${draft.conflict === null
        ? nothing
        : html`<section class="conflict">
            <sl-alert variant="warning" open>
            <sl-icon slot="icon" name="exclamation-mark"></sl-icon>
            <strong>Conflict</strong>
            <fmn-side-by-side
              leftLabel="Your text"
              .leftText=${scopeText({
                message: messageToSave(draft),
                implies: parseIdList(draft.impliesText),
                triggers: draft.triggers,
              })}
              rightLabel="Current on server"
              .rightText=${scopeText(draft.conflict)}
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
          </section>`}
    `;
  }

  private renderScope(scope: ScopeDoc): TemplateResult {
    const draft = this.draft ?? draftOf(scope);
    return html`
      <div class="infobox">
        ${this.creating
          ? html`<div class="field field-id">
              <sl-input
                size="small"
                label="Id"
                help-text="The name of the file under scopes/, without .yaml"
                value=${this.newId}
                @sl-input=${(event: Event) => {
                  this.newId = (event.target as HTMLInputElement).value;
                }}
              ></sl-input>
            </div>`
          : nothing}
        ${this.renderField(
          'message',
          'Message',
          draft.scopeMessage.trim() === ''
            ? html`<span class="empty">none</span>`
            : html`<span class="value-text">${draft.scopeMessage}</span>`,
          () => html`<sl-textarea
            size="small"
            label="Message"
            rows="3"
            value=${draft.scopeMessage}
            @sl-input=${(event: Event) => {
              this.change({ scopeMessage: (event.target as HTMLInputElement).value });
            }}
          ></sl-textarea>`,
        )}
        ${this.renderField(
          'implies',
          'Implies',
          parseIdList(draft.impliesText).length === 0
            ? html`<span class="empty">none</span>`
            : html`<span class="chips"
                >${parseIdList(draft.impliesText).map(
                  (other) =>
                    html`<a href=${paths.scope(other)}
                      ><sl-badge variant="neutral" pill>${other}</sl-badge></a
                    >`,
                )}</span
              >`,
          () => html`<fmn-tag-field
            label="Implies"
            placeholder="Add a scope"
            .value=${parseIdList(draft.impliesText)}
            .suggestions=${this.scopeOptions.value ?? []}
            @fmn-tags-change=${(event: CustomEvent<TagsChange>) =>
              this.change({ impliesText: event.detail.value.join(', ') })}
          ></fmn-tag-field>`,
        )}
        ${this.creating
          ? nothing
          : html`<div class="field">
                <span class="field-label">Version</span>
                <div class="field-value"><code>${scope.version}</code></div>
              </div>
              <div class="field">
                <sl-details summary="Delete" ?open=${this.deleteOpen}>
                  <p class="muted">
                    The file is removed in a commit; the history keeps it. A scope that a
                    memory still lists cannot be removed.
                  </p>
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
                    @click=${() => void this.deleteScope(scope)}
                    >Delete scope</sl-button
                  >
                  <fmn-validation-errors .errors=${this.deleteErrors}></fmn-validation-errors>
                  ${this.deleteFailure === null
                    ? nothing
                    : html`<p class="failure" role="alert">${this.deleteFailure}</p>`}
                </sl-details>
              </div>`}
      </div>
      ${this.renderTriggers(scope, draft)}
      ${this.creating
        ? nothing
        : html`<h2>Memories</h2>
            ${this.renderMemories()}
            <fmn-trigger-test
              heading="Trigger test"
              idPrefix="scope-test"
              .machines=${this.machines.value ?? []}
            ></fmn-trigger-test>`}
    `;
  }

  /** A scope with no file: what the index says about it, and its memories. */
  private renderWithoutFile(row: ScopeRow): TemplateResult {
    return html`
      <div class="infobox">
        <div class="field">
          <span class="field-label">Kind</span>
          <div class="field-value"><span class="value-text">${row.kind}</span></div>
        </div>
        ${row.name === null
          ? nothing
          : html`<div class="field">
              <span class="field-label">Name</span>
              <div class="field-value"><span class="value-text">${row.name}</span></div>
            </div>`}
      </div>
      <h2>Memories</h2>
      ${this.renderMemories()}
    `;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name">
          <sl-icon name="folder"></sl-icon>
          <span>${this.creating ? 'new scope' : 'scope'}</span>
        </div>
        <h1>${this.creating ? (this.newId === '' ? 'New scope' : this.newId) : this.scopeId}</h1>
      </header>
      ${gate(this.row.state, (row) =>
        row === null
          ? html`<p class="empty">No scope has this id</p>`
          : row.file === null
            ? this.renderWithoutFile(row)
            : this.renderScope(row.file),
      )}
    `;
  }
}

customElements.define('fmn-scope-page', FmnScopePage);
