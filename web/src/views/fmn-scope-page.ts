import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { RequestFailed, api } from '../api/client';
import type { MemorySummary, ScopeDoc, Trigger, ValidationError } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { frontendAuthor } from '../model/author';
import { parseIdList } from '../model/triggers';
import { announceStoreChange, onStoreChange } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-kind-icon';
import '../components/fmn-side-by-side';
import '../components/fmn-trigger-rows';
import '../components/fmn-trigger-test';
import '../components/fmn-validation-errors';

interface ScopeDraft {
  baseVersion: string;
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
function scopeText(scope: { implies: string[]; triggers: Trigger[] }): string {
  const lines = [`implies: ${scope.implies.join(', ')}`];
  for (const trigger of scope.triggers) {
    lines.push(
      `trigger: ${trigger.on} ${trigger.pattern}${trigger.machine ? ` @${trigger.machine}` : ''}`,
    );
  }
  return lines.join('\n');
}

function triggersEqual(left: Trigger[], right: Trigger[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function missingStatus(error: Error): 'failed' | 'missing' {
  return error instanceof RequestFailed && error.status === 404 ? 'missing' : 'failed';
}

/** One scope: its type, what it implies, its triggers and its memories, edited in place. */
export class FmnScopePage extends PageElement {
  static override properties: PropertyDeclarations = {
    scopeId: { type: String },
    draft: { state: true },
    editing: { state: true },
  };

  scopeId = '';

  private readonly scope = new Resource<ScopeDoc>(() => this.requestUpdate(), {
    classify: missingStatus,
  });
  private readonly memories = new Resource<MemorySummary[]>(() => this.requestUpdate());

  private draft: ScopeDraft | null = null;
  private editing: string | null = null;

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
      this.draft = null;
      void this.scope.load(() => api.scope(this.scopeId));
      void this.memories.load(() => api.memoryIndex({ scope: this.scopeId }));
    }
    const scope = this.scope.value;
    if (scope !== null && (this.draft === null || this.draft.baseVersion !== scope.version)) {
      this.draft = draftOf(scope);
    }
  }

  private change(change: Partial<ScopeDraft>): void {
    if (this.draft === null) return;
    this.draft = { ...this.draft, ...change };
  }

  private dirty(scope: ScopeDoc, draft: ScopeDraft): boolean {
    if (parseIdList(draft.impliesText).join(',') !== scope.implies.join(',')) return true;
    return !triggersEqual(draft.triggers, scope.triggers);
  }

  private async save(): Promise<void> {
    const draft = this.draft;
    if (draft === null) return;
    this.change({ saving: true, errors: [], conflict: null, failure: null });
    try {
      const outcome = await api.putScope(this.scopeId, {
        implies: parseIdList(draft.impliesText),
        triggers: draft.triggers,
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
    const editing = this.editing === 'triggers';
    return html`
      <h2>
        Triggers
        <sl-icon-button
          name=${editing ? 'x' : 'pencil'}
          label=${editing ? 'Close Triggers' : 'Edit Triggers'}
          @click=${() => {
            this.editing = editing ? null : 'triggers';
          }}
        ></sl-icon-button>
      </h2>
      ${editing
        ? html`<fmn-trigger-rows
            .triggers=${draft.triggers}
            @fmn-triggers-change=${(event: CustomEvent<{ triggers: Trigger[] }>) =>
              this.change({ triggers: event.detail.triggers })}
          ></fmn-trigger-rows>`
        : draft.triggers.length === 0
          ? html`<p class="empty">No triggers</p>`
          : html`<div class="table-wrap">
              <table class="data">
                <thead>
                  <tr>
                    <th scope="col">Field</th>
                    <th scope="col">Pattern</th>
                    <th scope="col">Machine</th>
                  </tr>
                </thead>
                <tbody>
                  ${draft.triggers.map(
                    (trigger) => html`<tr>
                      <td class="nowrap">${trigger.on}</td>
                      <td><code>${trigger.pattern}</code></td>
                      <td>${trigger.machine ?? ''}</td>
                    </tr>`,
                  )}
                </tbody>
              </table>
            </div>`}
      ${this.dirty(scope, draft)
        ? html`<fmn-commit-bar
            .message=${draft.message}
            .saving=${draft.saving}
            saveLabel="Save"
            @fmn-message-change=${(event: CustomEvent<{ message: string }>) =>
              this.change({ message: event.detail.message })}
            @fmn-save=${() => void this.save()}
            @fmn-discard=${() => {
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
          () => html`<sl-input
            size="small"
            value=${draft.impliesText}
            help-text="Comma separated"
            @sl-input=${(event: Event) =>
              this.change({ impliesText: (event.target as HTMLInputElement).value })}
          ></sl-input>`,
        )}
        <div class="field">
          <span class="field-label">Version</span>
          <div class="field-value"><code>${scope.version}</code></div>
        </div>
      </div>
      ${this.renderTriggers(scope, draft)}
      <h2>Memories</h2>
      ${this.renderMemories()}
      <fmn-trigger-test heading="Trigger test" idPrefix="scope-test"></fmn-trigger-test>
    `;
  }

  /** A scope with no file: its memories are all there is to show. */
  private renderImplicit(): TemplateResult {
    return html`
      <div class="infobox">
        <div class="field">
          <span class="field-label">File</span>
          <div class="field-value"><span class="value-text">none</span></div>
        </div>
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
          <span>scope</span>
        </div>
        <h1>${this.scopeId}</h1>
      </header>
      ${gate(
        this.scope.state,
        (scope) => this.renderScope(scope),
        () => this.renderImplicit(),
      )}
    `;
  }
}

customElements.define('fmn-scope-page', FmnScopePage);
