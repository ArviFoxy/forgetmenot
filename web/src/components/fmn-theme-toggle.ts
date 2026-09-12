import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import { setScheme, storedScheme, type ColourScheme } from '../theme';

const order: ColourScheme[] = ['system', 'light', 'dark'];

const icons: Record<ColourScheme, string> = {
  system: 'bi-circle-half',
  light: 'bi-sun',
  dark: 'bi-moon-stars',
};

/** Light, dark, or the system setting. */
export class FmnThemeToggle extends PageElement {
  static override properties: PropertyDeclarations = {
    scheme: { state: true },
  };

  private scheme: ColourScheme = storedScheme();

  private next(): void {
    const at = order.indexOf(this.scheme);
    const scheme = order[(at + 1) % order.length] ?? 'system';
    this.scheme = scheme;
    setScheme(scheme);
  }

  override render(): TemplateResult {
    const label = `Colour scheme: ${this.scheme}`;
    return html`<button
      type="button"
      class="icon-button"
      aria-label=${label}
      title=${label}
      @click=${() => this.next()}
    >
      <i class="bi ${icons[this.scheme]}"></i>
    </button>`;
  }
}

customElements.define('fmn-theme-toggle', FmnThemeToggle);
