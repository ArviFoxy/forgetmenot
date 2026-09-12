import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { diffLines } from 'diff';
import { PageElement } from '../lib/element';

function markedLines(own: string, other: string): { text: string; changed: boolean }[] {
  const rows: { text: string; changed: boolean }[] = [];
  for (const part of diffLines(other, own)) {
    if (part.removed) continue;
    for (const line of part.value.replace(/\n$/, '').split('\n')) {
      rows.push({ text: line, changed: part.added === true });
    }
  }
  return rows;
}

/** Two versions of one text, the lines that differ marked on each side. */
export class FmnSideBySide extends PageElement {
  static override properties: PropertyDeclarations = {
    leftLabel: { type: String },
    leftText: { type: String },
    rightLabel: { type: String },
    rightText: { type: String },
  };

  leftLabel = '';
  leftText = '';
  rightLabel = '';
  rightText = '';

  override render(): TemplateResult {
    const sides = [
      { label: this.leftLabel, rows: markedLines(this.leftText, this.rightText) },
      { label: this.rightLabel, rows: markedLines(this.rightText, this.leftText) },
    ];
    return html`<div class="side-by-side">
      ${sides.map(
        (side) => html`<section>
          <h3>${side.label}</h3>
          <pre class="diff">${side.rows.map(
            (row) =>
              html`<code class=${row.changed ? 'diff-changed' : 'diff-context'}
                >${row.text === '' ? ' ' : row.text}</code
              >`,
          )}</pre>
        </section>`,
      )}
    </div>`;
  }
}

customElements.define('fmn-side-by-side', FmnSideBySide);
