// The body editor. It edits the markdown source, because the text it saves is the
// file the store commits: a round trip through a rich-text model would rewrite line
// wrapping and escape `[[name]]` links, and every save would commit that rewrite.
// The source is styled like the document instead: heading sizes, proportional text,
// monospace only where the markup is code.

import { html, type PropertyValues, type TemplateResult, type PropertyDeclarations } from 'lit';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap, drawSelection, highlightSpecialChars } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { markdown } from '@codemirror/lang-markdown';
import { tags } from '@lezer/highlight';
import { PageElement } from '../lib/element';

// Colours and fonts are Pico's, so the editor follows the page's colour scheme
// without a theme of its own for each.
const documentTheme = EditorView.theme({
  '&': {
    color: 'var(--pico-color)',
    backgroundColor: 'var(--pico-form-element-background-color)',
    border: 'var(--pico-border-width) solid var(--pico-form-element-border-color)',
    borderRadius: 'var(--pico-border-radius)',
    fontFamily: 'var(--pico-font-family-sans-serif)',
    fontSize: '1rem',
  },
  '&.cm-focused': {
    outline: 'none',
    borderColor: 'var(--pico-form-element-active-border-color)',
    boxShadow: '0 0 0 var(--pico-outline-width) var(--pico-form-element-focus-color)',
  },
  '.cm-content': {
    padding: '1rem 1.25rem',
    lineHeight: '1.6',
    caretColor: 'var(--pico-primary)',
    fontFamily: 'var(--pico-font-family-sans-serif)',
  },
  '.cm-line': { padding: '0' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--pico-primary)' },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--pico-primary-focus)',
  },
  '.cm-scroller': { overflow: 'auto', fontFamily: 'inherit' },
});

const documentHighlight = HighlightStyle.define([
  { tag: tags.heading1, fontSize: '1.6em', fontWeight: '700', lineHeight: '1.3' },
  { tag: tags.heading2, fontSize: '1.35em', fontWeight: '700', lineHeight: '1.3' },
  { tag: tags.heading3, fontSize: '1.15em', fontWeight: '700' },
  { tag: [tags.heading4, tags.heading5, tags.heading6], fontWeight: '700' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: tags.link, color: 'var(--pico-primary)' },
  { tag: tags.url, color: 'var(--pico-primary)', textDecoration: 'underline' },
  { tag: tags.monospace, fontFamily: 'var(--pico-font-family-monospace)', fontSize: '0.9em' },
  { tag: tags.quote, color: 'var(--pico-muted-color)', fontStyle: 'italic' },
  { tag: [tags.list, tags.processingInstruction], color: 'var(--pico-muted-color)' },
  { tag: tags.contentSeparator, color: 'var(--pico-muted-color)' },
]);

/** Fired on every keystroke, carrying the whole text. */
export type SourceChange = CustomEvent<{ value: string }>;

export class FmnSourceEditor extends PageElement {
  static override properties: PropertyDeclarations = {
    value: { type: String },
    label: { type: String },
    editorId: { type: String },
    resetKey: { type: String },
  };

  /** The text to start from. The editor owns the text after that. */
  value = '';
  label = 'Body';
  editorId = 'memory-body';
  /**
   * When this changes, the editor takes `value` again. It exists because the text
   * must not be pushed back in on every keystroke: a keystroke that lands between
   * the change event and the property write-back would be undone by it.
   */
  resetKey = '';

  private view: EditorView | null = null;

  override render(): TemplateResult {
    return html`
      <label for=${this.editorId}>${this.label}</label>
      <div class="source-editor" id=${this.editorId}></div>
    `;
  }

  override firstUpdated(): void {
    const host = this.querySelector('.source-editor');
    if (host === null) return;
    this.view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: this.value,
        extensions: [
          history(),
          drawSelection(),
          highlightSpecialChars(),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          markdown(),
          syntaxHighlighting(documentHighlight),
          EditorView.lineWrapping,
          documentTheme,
          EditorView.updateListener.of((update) => {
            if (!update.docChanged) return;
            const value = update.state.doc.toString();
            this.dispatchEvent(
              new CustomEvent('fmn-source-change', { detail: { value }, bubbles: true, composed: true }),
            );
          }),
        ],
      }),
    });
  }

  override updated(changed: PropertyValues<this>): void {
    // Only a new reset key replaces the document: that is a reload or a discarded
    // edit, not the editor's own typing.
    if (!changed.has('resetKey') || this.view === null) return;
    const current = this.view.state.doc.toString();
    if (current === this.value) return;
    this.view.dispatch({ changes: { from: 0, to: current.length, insert: this.value } });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.view?.destroy();
    this.view = null;
  }
}

customElements.define('fmn-source-editor', FmnSourceEditor);
