import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { ContextPrompt, PromptMode } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { formatBytes } from '../model/units';
import { navigate } from '../navigation';
import { paths, promptModeFromSearch } from '../routes';

/** The label of each mode, and the text shown when that mode delivers nothing. */
const modes: { mode: PromptMode; label: string; empty: string }[] = [
  { mode: 'due', label: 'Due now', empty: 'Nothing due' },
  { mode: 'all', label: 'Everything active', empty: 'Nothing active' },
];

/**
 * The text one context would be given: what its next hook event delivers, or
 * everything its active scopes hold. The server renders it and records nothing,
 * so opening this page does not change what the context is owed.
 */
export class FmnContextPromptView extends PageElement {
  static override properties: PropertyDeclarations = {
    contextKey: { type: String },
    promptMode: { state: true },
  };

  contextKey = '';
  promptMode: PromptMode = promptModeFromSearch();

  private readonly prompt = new Resource<ContextPrompt>(() => this.requestUpdate());
  private loaded = '';

  override updated(): void {
    const wanted = `${this.contextKey}?${this.promptMode}`;
    if (this.contextKey === '' || this.loaded === wanted) return;
    this.loaded = wanted;
    void this.prompt.load(() => api.contextPrompt(this.contextKey, this.promptMode));
  }

  /** The address carries the mode, so a reload shows the text that is on screen. */
  private show(mode: PromptMode): void {
    if (mode === this.promptMode) return;
    this.promptMode = mode;
    navigate(paths.contextPrompt(this.contextKey, mode), { replace: true });
  }

  /**
   * The context's name, or its key while the name is still being read and for a
   * context nothing but the key is known about.
   */
  private heading(): string {
    const name = this.prompt.value?.name ?? '';
    return name === '' ? this.contextKey : name;
  }

  private renderText(prompt: ContextPrompt): TemplateResult {
    const empty = modes.find((each) => each.mode === prompt.mode)?.empty ?? '';
    return prompt.text === ''
      ? html`<p class="empty">${empty}</p>`
      : html`<pre class="prompt">${prompt.text}</pre>`;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name">
          <sl-icon name="activity"></sl-icon><a href=${paths.contexts()}>${this.contextKey}</a>
        </div>
        <h1>${this.heading()}</h1>
        <div class="actions">
          <sl-radio-group
            size="small"
            value=${this.promptMode}
            @sl-change=${(event: Event) =>
              this.show((event.target as HTMLInputElement).value as PromptMode)}
          >
            ${modes.map(
              (each) => html`<sl-radio-button value=${each.mode}>${each.label}</sl-radio-button>`,
            )}
          </sl-radio-group>
        </div>
      </header>
      ${gate(
        this.prompt.state,
        (prompt) => html`
          <p class="muted prompt-size">
            ${formatBytes(prompt.bytes)} · ~${prompt.tokens_estimate} tokens
          </p>
          ${this.renderText(prompt)}
        `,
      )}
    `;
  }
}

customElements.define('fmn-context-prompt-view', FmnContextPromptView);
