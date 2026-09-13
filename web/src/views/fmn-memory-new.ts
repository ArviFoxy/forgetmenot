import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import { Resource } from '../lib/resource';
import { suggestScopes } from '../model/tagField';
import type { MemoryKind, MemorySource, ValidationError } from '../api/types';
import { PageElement } from '../lib/element';
import { frontendAuthor } from '../model/author';
import { memoryKinds, memorySources, parseIdList } from '../model/triggers';
import { announceStoreChange, navigate } from '../navigation';
import { paths, scopeFromSearch } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-markdown-editor';
import '../components/fmn-tag-field';
import '../components/fmn-validation-errors';
import type { BodyChange } from '../components/fmn-markdown-editor';
import type { TagsChange } from '../components/fmn-tag-field';

/** Creates a memory: its id is its path under memories/. */
export class FmnMemoryNew extends PageElement {
  static override properties: PropertyDeclarations = {
    newId: { state: true },
    description: { state: true },
    kind: { state: true },
    scopesText: { state: true },
    source: { state: true },
    body: { state: true },
    message: { state: true },
    saving: { state: true },
    errors: { state: true },
    failure: { state: true },
  };

  private newId = '';
  private description = '';
  private kind: MemoryKind = 'knowledge';
  private scopesText = scopeFromSearch();
  private source: MemorySource = 'user';
  private body = '';
  private message = '';
  private saving = false;
  private errors: ValidationError[] = [];
  private failure: string | null = null;

  private readonly scopeOptions = new Resource<string[]>(() => this.requestUpdate());
  private loadedOptions = false;

  override updated(): void {
    if (this.loadedOptions) return;
    this.loadedOptions = true;
    void this.scopeOptions.load(async () => {
      const [scopes, contexts] = await Promise.all([api.scopeIndex(), api.contexts()]);
      return suggestScopes(scopes, contexts, parseIdList(this.scopesText));
    });
  }

  private async create(): Promise<void> {
    this.saving = true;
    this.errors = [];
    this.failure = null;
    try {
      const outcome = await api.createMemory({
        id: this.newId,
        description: this.description,
        kind: this.kind,
        scopes: parseIdList(this.scopesText),
        source: this.source,
        body: this.body,
        author: frontendAuthor,
        message: this.message,
      });
      if (outcome.kind === 'written') {
        announceStoreChange();
        navigate(paths.memory(this.newId));
        return;
      }
      if (outcome.kind === 'invalid') this.errors = outcome.failure.errors;
      else this.failure = `a memory with the id ${this.newId} already exists`;
    } catch (caught) {
      this.failure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.saving = false;
    }
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="plus"></sl-icon><span>new memory</span></div>
        <h1>New memory</h1>
      </header>
      <div class="field-grid">
        <sl-input
          size="small"
          label="Id"
          help-text="The path under memories/, without .md"
          value=${this.newId}
          @sl-input=${(event: Event) => {
            this.newId = (event.target as HTMLInputElement).value;
          }}
        ></sl-input>
        <sl-input
          size="small"
          label="Description"
          help-text="The line the agent sees in an index"
          value=${this.description}
          @sl-input=${(event: Event) => {
            this.description = (event.target as HTMLInputElement).value;
          }}
        ></sl-input>
        <sl-select
          size="small"
          label="Kind"
          value=${this.kind}
          @sl-change=${(event: Event) => {
            this.kind = (event.target as HTMLInputElement).value as MemoryKind;
          }}
        >
          ${memoryKinds.map((kind) => html`<sl-option value=${kind}>${kind}</sl-option>`)}
        </sl-select>
        <fmn-tag-field
          label="Scopes"
          showLabel
          placeholder="Add a scope"
          .value=${parseIdList(this.scopesText)}
          .suggestions=${this.scopeOptions.value ?? []}
          @fmn-tags-change=${(event: CustomEvent<TagsChange>) => {
            this.scopesText = event.detail.value.join(', ');
          }}
        ></fmn-tag-field>
        <sl-select
          size="small"
          label="Source"
          value=${this.source}
          @sl-change=${(event: Event) => {
            this.source = (event.target as HTMLInputElement).value as MemorySource;
          }}
        >
          ${memorySources.map((source) => html`<sl-option value=${source}>${source}</sl-option>`)}
        </sl-select>
      </div>

      <h2>Body</h2>
      <fmn-markdown-editor
        placeholder="Write the memory. Type / for blocks."
        @fmn-body-change=${(event: CustomEvent<BodyChange>) => {
          this.body = event.detail.value;
        }}
      ></fmn-markdown-editor>

      <fmn-commit-bar
        label="New memory"
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

customElements.define('fmn-memory-new', FmnMemoryNew);
