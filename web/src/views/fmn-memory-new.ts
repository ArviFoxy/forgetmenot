import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { MemoryKind, MemorySource, ValidationError } from '../api/types';
import { PageElement } from '../lib/element';
import { frontendAuthor } from '../model/memoryBody';
import { memoryKinds, memorySources, parseIdList } from '../model/triggers';
import { announceStoreChange, navigate } from '../navigation';
import { paths } from '../routes';
import '../components/fmn-commit-bar';
import '../components/fmn-source-editor';
import '../components/fmn-validation-errors';

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
  private scopesText = '';
  private source: MemorySource = 'user';
  private body = '';
  private message = '';
  private saving = false;
  private errors: ValidationError[] = [];
  private failure: string | null = null;

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
        <h1>New memory</h1>
      </header>
      <div class="field-grid">
        <label for="memory-id">Id</label>
        <input
          id="memory-id"
          type="text"
          required
          .value=${this.newId}
          @input=${(event: Event) => {
            this.newId = (event.target as HTMLInputElement).value;
          }}
        />

        <label for="memory-description">Description</label>
        <input
          id="memory-description"
          type="text"
          required
          .value=${this.description}
          @input=${(event: Event) => {
            this.description = (event.target as HTMLInputElement).value;
          }}
        />

        <label for="memory-kind">Kind</label>
        <select
          id="memory-kind"
          @change=${(event: Event) => {
            this.kind = (event.target as HTMLSelectElement).value as MemoryKind;
          }}
        >
          ${memoryKinds.map(
            (kind) => html`<option value=${kind} ?selected=${kind === this.kind}>${kind}</option>`,
          )}
        </select>

        <label for="memory-scopes">Scopes</label>
        <input
          id="memory-scopes"
          type="text"
          .value=${this.scopesText}
          @input=${(event: Event) => {
            this.scopesText = (event.target as HTMLInputElement).value;
          }}
        />

        <label for="memory-source">Source</label>
        <select
          id="memory-source"
          @change=${(event: Event) => {
            this.source = (event.target as HTMLSelectElement).value as MemorySource;
          }}
        >
          ${memorySources.map(
            (source) =>
              html`<option value=${source} ?selected=${source === this.source}>${source}</option>`,
          )}
        </select>
      </div>

      <fmn-source-editor
        label="Body"
        .value=${this.body}
        @fmn-source-change=${(event: CustomEvent<{ value: string }>) => {
          this.body = event.detail.value;
        }}
      ></fmn-source-editor>

      <fmn-commit-bar
        fieldId="commit-message"
        saveLabel="Create"
        .message=${this.message}
        .saving=${this.saving}
        cancelHref=${paths.home()}
        @fmn-message-change=${(event: CustomEvent<{ message: string }>) => {
          this.message = event.detail.message;
        }}
        @fmn-save=${() => void this.create()}
      ></fmn-commit-bar>

      <fmn-validation-errors .errors=${this.errors}></fmn-validation-errors>
      ${this.failure === null ? nothing : html`<p class="failure" role="alert">${this.failure}</p>`}
    `;
  }
}

customElements.define('fmn-memory-new', FmnMemoryNew);
