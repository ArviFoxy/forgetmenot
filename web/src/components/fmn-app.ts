import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import { currentPath, onLocationChange } from '../navigation';
import { paths, resolve, type RouteName, type RouteView } from '../routes';
import './fmn-sidebar';
import './fmn-theme-toggle';
import '../views/fmn-overview-view';
import '../views/fmn-memory-page';
import '../views/fmn-memory-new';
import '../views/fmn-scope-page';
import '../views/fmn-contexts-view';
import '../views/fmn-review-view';
import '../views/fmn-stats-view';
import '../views/fmn-trigger-test-view';
import '../views/fmn-unknown-view';

interface Section {
  name: RouteName;
  href: string;
  label: string;
}

const sections: Section[] = [
  { name: 'memoryNew', href: paths.memoryNew(), label: 'New memory' },
  { name: 'contexts', href: paths.contexts(), label: 'Contexts' },
  { name: 'review', href: paths.review(), label: 'Review' },
  { name: 'stats', href: paths.stats(), label: 'Statistics' },
  { name: 'triggerTest', href: paths.triggerTest(), label: 'Trigger test' },
];

/** Every property any view takes, so a reused element never keeps a stale one. */
const viewProperties = ['memoryId', 'scopeId', 'mode', 'oid'];

/** The shell: the bar, the hierarchy, and the view for the current address. */
export class FmnApp extends PageElement {
  static override properties: PropertyDeclarations = {
    view: { state: true },
    navOpen: { state: true },
  };

  private view: RouteView = resolve(currentPath());
  private navOpen = false;

  private element: HTMLElement | null = null;
  private stopListening: (() => void) | null = null;

  override connectedCallback(): void {
    super.connectedCallback();
    this.stopListening = onLocationChange(() => {
      this.view = resolve(currentPath());
      this.navOpen = false;
      this.querySelector('details.app-menu-compact')?.removeAttribute('open');
      window.scrollTo({ top: 0 });
    });
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.stopListening?.();
    this.stopListening = null;
  }

  /** The element that shows the address, reused while the address keeps the same view. */
  private viewElement(): HTMLElement {
    if (this.element === null || this.element.localName !== this.view.tag) {
      this.element = document.createElement(this.view.tag);
    }
    const target = this.element as unknown as Record<string, unknown>;
    for (const name of viewProperties) target[name] = this.view.properties[name] ?? '';
    return this.element;
  }

  private menuLinks(): TemplateResult[] {
    return sections.map(
      (section) => html`<li>
        <a href=${section.href} aria-current=${this.view.name === section.name ? 'page' : 'false'}>
          ${section.label}
        </a>
      </li>`,
    );
  }

  override render(): TemplateResult {
    return html`
      <header class="app-bar">
        <button
          type="button"
          class="icon-button nav-toggle"
          aria-label="Navigation"
          aria-expanded=${this.navOpen ? 'true' : 'false'}
          @click=${() => {
            this.navOpen = !this.navOpen;
          }}
        >
          <i class="bi bi-list"></i>
        </button>
        <a class="brand" href=${paths.home()}>forgetmenot</a>
        <nav class="app-menu" aria-label="Sections">
          <ul>
            ${this.menuLinks()}
          </ul>
        </nav>
        <details class="dropdown app-menu-compact">
          <summary>Menu</summary>
          <ul>
            ${this.menuLinks()}
          </ul>
        </details>
        <fmn-theme-toggle></fmn-theme-toggle>
      </header>
      <div class="app-body">
        <fmn-sidebar ?open=${this.navOpen}></fmn-sidebar>
        <div
          class="backdrop"
          ?data-open=${this.navOpen}
          @click=${() => {
            this.navOpen = false;
          }}
        ></div>
        <main class="content">${this.viewElement()}</main>
      </div>
    `;
  }
}

customElements.define('fmn-app', FmnApp);
