import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { RequestFailed, api } from '../api/client';
import type { MemorySummary, ScopeDoc, ScopeType, Trigger, ValidationError } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { frontendAuthor } from '../model/memoryBody';
import { parseIdList, scopeTypes } from '../model/triggers';
import { announceStoreChange, navigate, onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-kind-icon';
import '../components/fmn-side-by-side';
import '../components/fmn-trigger-rows';
import '../components/fmn-trigger-test';
import '../components/fmn-validation-errors';

export type ScopeMode = 'view' | 'edit';

interface ScopeDraft {
  baseVersion: string;
  type: ScopeType;
  impliesText: string;
  triggers: Trigger[];
  message: string;
  saving: boolean;
  errors: ValidationError[];
  conflict: ScopeDoc | null;
  failure: string | null;
}

function draftOf(scope: ScopeDoc): ScopeDraft {
  return {
    baseVersion: scope.version,
    type: scope.type,
    impliesText: scope.implies.join(', '),
    triggers: scope.triggers,
    message: '',
    saving: false,
    errors: [],
    conflict: null,
    failure: null,
  };
}

/** The scope fields as one text, so the two sides of a conflict can be compared. */
function scopeText(scope: { type: string; implies: string[]; triggers: Trigger[] }): string {
  const lines = [`type: ${scope.type}`, `implies: ${scope.implies.join(', ')}`];
  for (const trigger of scope.triggers) {
    lines.push(
      `trigger: ${trigger.on} ${trigger.pattern}${trigger.machine ? ` @${trigger.machine}` : ''}`,
    );
  }
  return lines.join('\n');
}

function missingStatus(error: Error): 'failed' | 'missing' {
  return error instanceof RequestFailed && error.status === 404 ? 'missing' : 'failed';
}

/** The implicit scopes have no file, so the kind of scope is read from the id. */
function implicitType(id: string): string {
  if (id === 'global') return 'global';
  if (id.startsWith('machine:')) return 'machine';
  if (id.startsWith('session:')) return 'session';
  return 'unknown';
}

/** One scope: its type, what it implies, its triggers, and the memories in it. */
export class FmnScopePage extends PageElement {
  static override properties: PropertyDeclarations = {
    scopeId: { type: String },
    mode: { type: String },
    draft: { state: true },
  };

  scopeId = '';
  mode: ScopeMode = 'view';

  private readonly scope = new Resource<ScopeDoc>(() => this.requestUpdate(), {
    classify: missingStatus,
  });
  private readonly memories = new Resource<MemorySummary[]>(() => this.requestUpdate());

  private draft: ScopeDraft | null = null;

  private loadedId = '';
  private stopListening: (() => void) | null = null;

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
    if (this.scopeId !== '' && this.loadedId !== this.scopeId) {
      this.loadedId = this.scopeId;
      void this.scope.load(() => api.scope(this.scopeId));
      void this.memories.load(() => api.memoryIndex({ scope: this.scopeId, archived: true }));
    }
    const scope = this.scope.value;
    if (this.mode === 'edit' && scope !== null) {
      if (this.draft === null || this.draft.baseVersion !== scope.version) this.draft = draftOf(scope);
      return;
    }
    if (this.mode !== 'edit' && this.draft !== null) this.draft = null;
  }

  private change(change: Partial<ScopeDraft>): void {
    if (this.draft === null) return;
    this.draft = { ...this.draft, ...change };
  }

  private async save(): Promise<void> {
    const draft = this.draft;
    if (draft === null) return;
    this.change({ saving: true, errors: [], conflict: null, failure: null });
    try {
      const outcome = await api.putScope(this.scopeId, {
        type: draft.type,
        implies: parseIdList(draft.impliesText),
        triggers: draft.triggers,
        base_version: draft.baseVersion,
        author: frontendAuthor,
        message: draft.message,
      });
      if (outcome.kind === 'written') {
        this.draft = null;
        announceStoreChange();
        navigate(paths.scope(this.scopeId));
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

  /** A refused write is reported below the trigger rows, which may be off screen. */
  private async showWriteOutcome(): Promise<void> {
    await this.updateComplete;
    const reported = this.querySelector('.conflict, fmn-validation-errors table, p.failure');
    reported?.scrollIntoView?.({ block: 'center' });
  }

  private renderMemories(): TemplateResult {
    return gate(
      this.memories.state,
      (memories) =>
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

  private renderTriggers(triggers: Trigger[]): TemplateResult {
    if (triggers.length === 0) return html`<p class="empty">No triggers</p>`;
    return html`<figure>
      <table>
        <thead>
          <tr>
            <th scope="col">Field</th>
            <th scope="col">Pattern</th>
            <th scope="col">Machine</th>
          </tr>
        </thead>
        <tbody>
          ${triggers.map(
            (trigger) => html`<tr>
              <td>${trigger.on}</td>
              <td><code>${trigger.pattern}</code></td>
              <td>${trigger.machine ?? ''}</td>
            </tr>`,
          )}
        </tbody>
      </table>
    </figure>`;
  }

  private renderView(scope: ScopeDoc): TemplateResult {
    return html`
      <nav class="tabs" aria-label="Scope views">
        <ul>
          <li><a href=${paths.scope(this.scopeId)} aria-current="page">Scope</a></li>
          <li><a href=${paths.scopeEdit(this.scopeId)} aria-current="false">Edit</a></li>
        </ul>
      </nav>
      <dl class="inline-fields">
        <dt>Type</dt>
        <dd>${scope.type}</dd>
        <dt>Implies</dt>
        <dd>
          ${scope.implies.length === 0
            ? html`<span class="empty">none</span>`
            : scope.implies.map((other) => html`<a class="chip" href=${paths.scope(other)}>${other}</a>`)}
        </dd>
        <dt>Version</dt>
        <dd><code>${scope.version}</code></dd>
      </dl>
      <h2>Triggers</h2>
      ${this.renderTriggers(scope.triggers)}
      <h2>Memories</h2>
      ${this.renderMemories()}
      <fmn-trigger-test heading="Trigger test" idPrefix="scope-test"></fmn-trigger-test>
    `;
  }

  private renderEdit(): TemplateResult {
    const draft = this.draft;
    if (draft === null) return html`<p aria-busy="true">Loading</p>`;
    return html`
      <nav class="tabs" aria-label="Scope views">
        <ul>
          <li><a href=${paths.scope(this.scopeId)} aria-current="false">Scope</a></li>
          <li><a href=${paths.scopeEdit(this.scopeId)} aria-current="page">Edit</a></li>
        </ul>
      </nav>
      <div class="field-grid">
        <label for="scope-type">Type</label>
        <select
          id="scope-type"
          @change=${(event: Event) =>
            this.change({ type: (event.target as HTMLSelectElement).value as ScopeType })}
        >
          ${scopeTypes.map(
            (type) => html`<option value=${type} ?selected=${type === draft.type}>${type}</option>`,
          )}
        </select>

        <label for="scope-implies">Implies</label>
        <input
          id="scope-implies"
          type="text"
          .value=${draft.impliesText}
          @input=${(event: Event) =>
            this.change({ impliesText: (event.target as HTMLInputElement).value })}
        />
      </div>

      <h2>Triggers</h2>
      <fmn-trigger-rows
        .triggers=${draft.triggers}
        @fmn-triggers-change=${(event: CustomEvent<{ triggers: Trigger[] }>) =>
          this.change({ triggers: event.detail.triggers })}
      ></fmn-trigger-rows>

      <fmn-commit-bar
        fieldId="commit-message"
        saveLabel="Save"
        .message=${draft.message}
        .saving=${draft.saving}
        cancelHref=${paths.scope(this.scopeId)}
        @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
          this.change({ message: event.detail.message })}
        @fmn-save=${() => void this.save()}
      ></fmn-commit-bar>

      <fmn-validation-errors .errors=${draft.errors}></fmn-validation-errors>
      ${draft.failure === null ? nothing : html`<p class="failure" role="alert">${draft.failure}</p>`}
      ${draft.conflict === null
        ? nothing
        : html`<section class="conflict" role="alert">
            <h2>Conflict</h2>
            <fmn-side-by-side
              leftLabel="Your text"
              .leftText=${scopeText({
                type: draft.type,
                implies: parseIdList(draft.impliesText),
                triggers: draft.triggers,
              })}
              rightLabel="Current on server"
              .rightText=${scopeText(draft.conflict)}
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
          </section>`}
      <fmn-trigger-test heading="Trigger test" idPrefix="scope-test"></fmn-trigger-test>
    `;
  }

  /** A scope with no file: its memories and its triggers are all there is to show. */
  private renderImplicit(): TemplateResult {
    return html`
      <dl class="inline-fields">
        <dt>Type</dt>
        <dd>${implicitType(this.scopeId)}</dd>
        <dt>File</dt>
        <dd>none</dd>
      </dl>
      <h2>Memories</h2>
      ${this.renderMemories()}
    `;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <h1>${this.scopeId}</h1>
      </header>
      ${gate(
        this.scope.state,
        (scope) => (this.mode === 'edit' ? this.renderEdit() : this.renderView(scope)),
        () => this.renderImplicit(),
      )}
    `;
  }
}

customElements.define('fmn-scope-page', FmnScopePage);
