import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import { Resource } from '../lib/resource';
import type { Trigger, ValidationError } from '../api/types';
import { PageElement } from '../lib/element';
import { frontendAuthor } from '../model/author';
import { parseIdList } from '../model/triggers';
import { announceStoreChange, navigate } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-tag-field';
import '../components/fmn-trigger-rows';
import '../components/fmn-validation-errors';
import type { TagsChange } from '../components/fmn-tag-field';

/** Creates a scope: an id, what it implies, and the triggers that turn it on. */
export class FmnScopeNew extends PageElement {
  static override properties: PropertyDeclarations = {
    newId: { state: true },
    impliesText: { state: true },
    triggers: { state: true },
    message: { state: true },
    saving: { state: true },
    errors: { state: true },
    failure: { state: true },
  };

  private newId = '';
  private impliesText = '';
  private triggers: Trigger[] = [];
  private message = '';
  private saving = false;
  private errors: ValidationError[] = [];
  private failure: string | null = null;

  private readonly scopeOptions = new Resource<string[]>(() => this.requestUpdate());
  /** The machine names the trigger rows offer. */
  private readonly machines = new Resource<string[]>(() => this.requestUpdate());
  private loadedOptions = false;

  override updated(): void {
    if (this.loadedOptions) return;
    this.loadedOptions = true;
    void this.scopeOptions.load(async () => (await api.scopeIndex()).map((scope) => scope.id));
    void this.machines.load(() => api.machineIndex());
  }

  private async create(): Promise<void> {
    this.saving = true;
    this.errors = [];
    this.failure = null;
    try {
      const outcome = await api.createScope({
        id: this.newId,
        implies: parseIdList(this.impliesText),
        triggers: this.triggers,
        author: frontendAuthor,
        message: this.message,
      });
      if (outcome.kind === 'written') {
        announceStoreChange();
        navigate(paths.scope(this.newId));
        return;
      }
      if (outcome.kind === 'invalid') this.errors = outcome.failure.errors;
      else this.failure = `a scope with the id ${this.newId} already exists`;
    } catch (caught) {
      this.failure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.saving = false;
    }
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="folder"></sl-icon><span>new scope</span></div>
        <h1>New scope</h1>
      </header>
      <div class="field-grid">
        <sl-input
          size="small"
          label="Id"
          help-text="The name of the file under scopes/, without .yaml"
          value=${this.newId}
          @sl-input=${(event: Event) => {
            this.newId = (event.target as HTMLInputElement).value;
          }}
        ></sl-input>
        <fmn-tag-field
          label="Implies"
          showLabel
          placeholder="Scopes that are on whenever this one is"
          .value=${parseIdList(this.impliesText)}
          .suggestions=${this.scopeOptions.value ?? []}
          @fmn-tags-change=${(event: CustomEvent<TagsChange>) => {
            this.impliesText = event.detail.value.join(', ');
          }}
        ></fmn-tag-field>
      </div>

      <h2>Triggers</h2>
      <fmn-trigger-rows
        .triggers=${this.triggers}
        .machines=${this.machines.value ?? []}
        @fmn-triggers-change=${(event: CustomEvent<{ triggers: Trigger[] }>) => {
          this.triggers = event.detail.triggers;
        }}
      ></fmn-trigger-rows>

      <fmn-commit-bar
        label="New scope"
        saveLabel="Create"
        .message=${this.message}
        .saving=${this.saving}
        @fmn-message-change=${(event: CustomEvent<{ message: string }>) => {
          this.message = event.detail.message;
        }}
        @fmn-save=${() => void this.create()}
        @fmn-discard=${() => navigate(paths.home())}
      ></fmn-commit-bar>

      <fmn-validation-errors .errors=${this.errors}></fmn-validation-errors>
      ${this.failure === null ? nothing : html`<p class="failure" role="alert">${this.failure}</p>`}
    `;
  }
}

customElements.define('fmn-scope-new', FmnScopeNew);
