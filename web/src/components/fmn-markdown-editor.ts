// The body editor: Milkdown's Crepe, a complete markdown editor with its own
// toolbar, slash menu, link tooltip, code blocks and tables. It edits the rendered
// document, so editing looks like reading.

import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { Crepe } from '@milkdown/crepe';
import { PageElement } from '../lib/element';
import { fromEditor } from '../model/memoryDraft';

export interface BodyChange {
  /** The markdown the editor holds now. */
  value: string;
}

export class FmnMarkdownEditor extends PageElement {
  static override properties: PropertyDeclarations = {
    value: { type: String },
    resetKey: { type: String },
    placeholder: { type: String },
  };

  /** The markdown the editor starts from. The editor owns the text after that. */
  value = '';
  /** When this changes, the editor is rebuilt from `value`. */
  resetKey = '';
  placeholder = 'Write the memory. Type / for blocks.';

  private crepe: Crepe | null = null;
  private live = false;
  private building: Promise<void> | null = null;

  override render(): TemplateResult {
    return html`<div class="editor-host"></div>`;
  }

  override firstUpdated(): void {
    void this.build();
  }

  override updated(changed: Map<PropertyKey, unknown>): void {
    if (!changed.has('resetKey') || this.crepe === null) return;
    void this.rebuild();
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    void this.crepe?.destroy();
    this.crepe = null;
  }

  private async build(): Promise<void> {
    const root = this.querySelector('.editor-host');
    if (root === null) return;
    const crepe = new Crepe({
      root,
      defaultValue: this.value,
      features: {
        [Crepe.Feature.Latex]: false,
        [Crepe.Feature.ImageBlock]: false,
      },
      featureConfigs: {
        [Crepe.Feature.Placeholder]: { text: this.placeholder },
      },
    });
    crepe.on((api) => {
      api.markdownUpdated(() => {
        if (!this.live) return;
        this.dispatchEvent(
          new CustomEvent<BodyChange>('fmn-body-change', {
            detail: { value: this.markdown() },
            bubbles: true,
          }),
        );
      });
    });
    this.crepe = crepe;
    this.building = crepe.create().then(() => {
      // The first parse of the document counts as no edit.
      window.setTimeout(() => {
        this.live = true;
      }, 0);
    });
    await this.building;
  }

  private async rebuild(): Promise<void> {
    this.live = false;
    await this.building;
    await this.crepe?.destroy();
    this.crepe = null;
    const host = this.querySelector('.editor-host');
    if (host !== null) host.replaceChildren();
    await this.build();
  }

  /** The markdown the editor holds, with `[[name]]` links left as they were typed. */
  markdown(): string {
    if (this.crepe === null) return this.value;
    return fromEditor(this.crepe.getMarkdown());
  }
}

customElements.define('fmn-markdown-editor', FmnMarkdownEditor);
