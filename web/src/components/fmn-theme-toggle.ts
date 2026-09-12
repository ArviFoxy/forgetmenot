import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import { setScheme, storedScheme, type ColourScheme } from '../theme';

const order: ColourScheme[] = ['system', 'light', 'dark'];

const icons: Record<ColourScheme, string> = {
  system: 'monitor-cog',
  light: 'sun',
  dark: 'moon',
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
    return html`<sl-tooltip content=${label} hoist>
      <sl-icon-button
        name=${icons[this.scheme]}
        label=${label}
        @click=${() => this.next()}
      ></sl-icon-button>
    </sl-tooltip>`;
  }
}

customElements.define('fmn-theme-toggle', FmnThemeToggle);
