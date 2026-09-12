import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import { currentPath, navigate, onLocationChange } from '../navigation';
import { paths, resolve, type RouteName, type RouteView } from '../routes';
import './fmn-sidebar';
import './fmn-theme-toggle';
import '../views/fmn-overview-view';
import '../views/fmn-memory-page';
import '../views/fmn-memory-new';
import '../views/fmn-scope-page';
import '../views/fmn-contexts-view';
import '../views/fmn-stats-view';
import '../views/fmn-unknown-view';

interface Section {
  name: RouteName;
  href: string;
  label: string;
  icon: string;
}

const sections: Section[] = [
  { name: 'home', href: paths.home(), label: 'Overview', icon: 'file-text' },
  { name: 'memoryNew', href: paths.memoryNew(), label: 'New memory', icon: 'plus' },
  { name: 'contexts', href: paths.contexts(), label: 'Contexts', icon: 'activity' },
  { name: 'stats', href: paths.stats(), label: 'Statistics', icon: 'chart-column' },
];

/** Every property any view takes, so a reused element never keeps a stale one. */
const viewProperties = ['memoryId', 'scopeId', 'mode', 'oid'];

const narrowQuery = '(max-width: 900px)';

/** The shell: the bar, the hierarchy, and the view for the current address. */
export class FmnApp extends PageElement {
  static override properties: PropertyDeclarations = {
    view: { state: true },
    navOpen: { state: true },
    narrow: { state: true },
  };

  private view: RouteView = resolve(currentPath());
  private navOpen = false;
  private narrow = window.matchMedia(narrowQuery).matches;

  private element: HTMLElement | null = null;
  private stopListening: (() => void)[] = [];

  override connectedCallback(): void {
    super.connectedCallback();
    const media = window.matchMedia(narrowQuery);
    const onResize = (): void => {
      this.narrow = media.matches;
    };
    media.addEventListener('change', onResize);
    this.stopListening = [
      onLocationChange(() => {
        this.view = resolve(currentPath());
        this.navOpen = false;
        window.scrollTo({ top: 0 });
      }),
      () => media.removeEventListener('change', onResize),
    ];
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    for (const stop of this.stopListening) stop();
    this.stopListening = [];
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

  override render(): TemplateResult {
    return html`
      <header class="app-bar">
        ${this.narrow
          ? html`<sl-icon-button
              class="nav-toggle"
              name="menu"
              label="Navigation"
              @click=${() => {
                this.navOpen = true;
              }}
            ></sl-icon-button>`
          : nothing}
        <a class="brand" href=${paths.home()}>forgetmenot</a>
        <nav class="app-menu" aria-label="Sections">
          ${sections.map(
            (section) => html`<a
              href=${section.href}
              aria-current=${this.view.name === section.name ? 'page' : 'false'}
            >
              <sl-icon name=${section.icon}></sl-icon>${section.label}
            </a>`,
          )}
        </nav>
        <sl-dropdown class="app-menu-compact" placement="bottom-end" hoist>
          <sl-button slot="trigger" size="small" caret>Menu</sl-button>
          <sl-menu
            @sl-select=${(event: CustomEvent<{ item: { value: string } }>) => {
              const href = event.detail.item.value;
              if (href !== '') navigate(href);
            }}
          >
            ${sections.map(
              (section) => html`<sl-menu-item value=${section.href}>
                <sl-icon slot="prefix" name=${section.icon}></sl-icon>${section.label}
              </sl-menu-item>`,
            )}
          </sl-menu>
        </sl-dropdown>
        <fmn-theme-toggle></fmn-theme-toggle>
      </header>
      <div class="app-body">
        ${this.narrow
          ? html`<sl-drawer
              class="nav-drawer"
              label="Scopes and memories"
              placement="start"
              ?open=${this.navOpen}
              @sl-after-hide=${() => {
                this.navOpen = false;
              }}
            >
              <fmn-sidebar></fmn-sidebar>
            </sl-drawer>`
          : html`<fmn-sidebar></fmn-sidebar>`}
        <main class="content">
          <div class="surface">${this.viewElement()}</div>
        </main>
      </div>
    `;
  }
}

customElements.define('fmn-app', FmnApp);
