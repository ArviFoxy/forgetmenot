// A tag field: the chosen ids as removable chips, a box to type in, and a list of
// the ids that match what is typed. The chips, the box, the list and its rows are
// Shoelace's tag, input, dropdown and menu; what is written here is which ids to
// offer and what the keys do.

import { html, type TemplateResult, type PropertyDeclarations } from 'lit';
import { PageElement } from '../lib/element';
import {
  addTag,
  cleanTag,
  filterSuggestions,
  removeLastTag,
  removeTag,
  setSingleTag,
} from '../model/tagField';

export interface TagsChange {
  value: string[];
}

export class FmnTagField extends PageElement {
  static override properties: PropertyDeclarations = {
    value: { attribute: false },
    suggestions: { attribute: false },
    label: { type: String },
    showLabel: { type: Boolean },
    single: { type: Boolean },
    placeholder: { type: String },
    text: { state: true },
    activeIndex: { state: true },
    open: { state: true },
  };

  value: string[] = [];
  suggestions: string[] = [];
  /** The name the field answers to; always set, shown only when asked for. */
  label = 'Tags';
  showLabel = false;
  /** One value rather than a list: a new choice replaces the one before it. */
  single = false;
  placeholder = 'Type to search, Enter to add';

  private text = '';
  private activeIndex = 0;
  private open = false;

  private emit(value: string[]): void {
    this.value = value;
    this.dispatchEvent(new CustomEvent<TagsChange>('fmn-tags-change', { detail: { value }, bubbles: true }));
  }

  private offered(): string[] {
    return filterSuggestions(this.suggestions, this.single ? [] : this.value, this.text);
  }

  private choose(id: string): void {
    this.emit(this.single ? setSingleTag(id) : addTag(this.value, id));
    this.text = '';
    this.activeIndex = 0;
    this.open = false;
    this.field()?.focus();
  }

  private field(): HTMLInputElement | null {
    return this.querySelector('sl-input') as HTMLInputElement | null;
  }

  private onKeyDown(event: KeyboardEvent): void {
    const matches = this.offered();
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      this.open = true;
      this.activeIndex = matches.length === 0 ? 0 : (this.activeIndex + 1) % matches.length;
      return;
    }
    if (event.key === 'ArrowUp') {
      event.preventDefault();
      this.activeIndex = matches.length === 0 ? 0 : (this.activeIndex + matches.length - 1) % matches.length;
      return;
    }
    if (event.key === 'Enter' || event.key === ',') {
      event.preventDefault();
      const active = this.open ? matches[this.activeIndex] : undefined;
      // The list wins when it is open; otherwise the typed id is taken as it is.
      this.choose(active ?? cleanTag(this.text));
      return;
    }
    if (event.key === 'Escape') {
      this.open = false;
      return;
    }
    if (event.key === 'Backspace' && cleanTag(this.text) === '' && this.value.length > 0) {
      event.preventDefault();
      this.emit(removeLastTag(this.value));
    }
  }

  override render(): TemplateResult {
    const matches = this.offered();
    const listOpen = this.open && matches.length > 0;
    return html`
      <div class="tag-field" ?data-labelled=${this.showLabel}>
        <div class="tag-chips">
          ${this.value.map(
            (tag) => html`<sl-tag
              size="small"
              removable
              @sl-remove=${() => this.emit(removeTag(this.value, tag))}
              >${tag}</sl-tag
            >`,
          )}
        </div>
        <sl-dropdown
          ?open=${listOpen}
          hoist
          placement="bottom-start"
          .containingElement=${this}
          @sl-after-hide=${() => {
            this.open = false;
          }}
        >
          <sl-input
            slot="trigger"
            size="small"
            autocomplete="off"
            placeholder=${this.placeholder}
            label=${this.label}
            .value=${this.text}
            @sl-input=${(event: Event) => {
              this.text = (event.target as HTMLInputElement).value;
              this.activeIndex = 0;
              this.open = true;
            }}
            @sl-focus=${() => {
              this.open = true;
            }}
            @keydown=${(event: KeyboardEvent) => this.onKeyDown(event)}
          ></sl-input>
          <sl-menu
            @sl-select=${(event: CustomEvent<{ item: { value: string } }>) =>
              this.choose(event.detail.item.value)}
          >
            ${matches.slice(0, 8).map(
              (id, index) => html`<sl-menu-item
                value=${id}
                class=${index === this.activeIndex ? 'active' : ''}
                >${id}</sl-menu-item
              >`,
            )}
          </sl-menu>
        </sl-dropdown>
      </div>
    `;
  }
}

customElements.define('fmn-tag-field', FmnTagField);
