import { html, nothing, type TemplateResult, type PropertyDeclarations } from 'lit';
import { api } from '../api/client';
import type { SettingValue, SettingsDoc } from '../api/types';
import { PageElement, gate } from '../lib/element';
import { Resource } from '../lib/resource';
import { frontendAuthor } from '../model/author';
import {
  changedKeys,
  parseNumberOrOff,
  settingRows,
  settingText,
  type SettingRow,
} from '../model/settings';
import '../components/fmn-commit-bar';
import '../components/fmn-side-by-side';
import '../components/fmn-tag-field';
import '../components/fmn-validation-errors';
import type { TagsChange } from '../components/fmn-tag-field';
import type { ValidationError } from '../api/types';

/**
 * The store's behaviour settings: what the schema says, with a control for each
 * type and the same commit bar as everywhere else. The server writes one key per
 * commit, so a save with two keys changed makes two commits with the same message.
 */
export class FmnSettingsView extends PageElement {
  static override properties: PropertyDeclarations = {
    edited: { state: true },
    message: { state: true },
    saving: { state: true },
    errors: { state: true },
    conflict: { state: true },
    failure: { state: true },
  };

  private readonly doc = new Resource<SettingsDoc>(() => this.requestUpdate());
  private loaded = false;

  /** The values the reader has changed, by key; anything else is the server's. */
  private edited: Record<string, SettingValue> = {};
  private message = '';
  private saving = false;
  private errors: ValidationError[] = [];
  private conflict: SettingsDoc | null = null;
  private failure: string | null = null;

  override updated(): void {
    if (this.loaded) return;
    this.loaded = true;
    void this.doc.load(() => api.settings());
  }

  private reload(): void {
    this.edited = {};
    this.message = '';
    this.conflict = null;
    this.errors = [];
    this.failure = null;
    void this.doc.load(() => api.settings());
  }

  private set(key: string, value: SettingValue): void {
    this.edited = { ...this.edited, [key]: value };
  }

  private valueOf(row: SettingRow): SettingValue {
    return Object.prototype.hasOwnProperty.call(this.edited, row.key)
      ? (this.edited[row.key] ?? null)
      : row.value;
  }

  private async save(doc: SettingsDoc): Promise<void> {
    const keys = changedKeys(doc, this.edited);
    this.saving = true;
    this.errors = [];
    this.conflict = null;
    this.failure = null;
    // One key per request, because that is what the server writes; each write
    // reports the version the next one starts from.
    let version = doc.version;
    try {
      for (const key of keys) {
        const outcome = await api.putSetting(key, {
          value: this.edited[key] ?? null,
          ...(version === null ? {} : { base_version: version }),
          author: frontendAuthor,
          message: this.message,
        });
        if (outcome.kind === 'conflict') {
          this.conflict = outcome.conflict.current;
          return;
        }
        if (outcome.kind === 'invalid') {
          this.errors = outcome.failure.errors;
          return;
        }
        version = outcome.response.version;
      }
      this.reload();
    } catch (caught) {
      this.failure = caught instanceof Error ? caught.message : String(caught);
    } finally {
      this.saving = false;
    }
  }

  private renderControl(row: SettingRow): TemplateResult {
    const value = this.valueOf(row);
    switch (row.control) {
      case 'switch':
        return html`<sl-switch
          class="setting-control"
          ?checked=${value === true}
          @sl-change=${(event: Event) =>
            this.set(row.key, (event.target as HTMLInputElement).checked)}
          >${value === true ? 'on' : 'off'}</sl-switch
        >`;
      case 'number-or-off':
        // An empty field is the value null, which is what turns the behaviour off.
        return html`<sl-input
          class="setting-control"
          size="small"
          type="number"
          min="0"
          placeholder="off"
          value=${value === null ? '' : String(value)}
          @sl-input=${(event: Event) =>
            this.set(row.key, parseNumberOrOff((event.target as HTMLInputElement).value, row.type))}
        ></sl-input>`;
      case 'number':
        return html`<sl-input
          class="setting-control"
          size="small"
          type="number"
          min="0"
          step=${row.type === 'number' ? 'any' : '1'}
          value=${value === null ? '' : String(value)}
          @sl-input=${(event: Event) =>
            this.set(
              row.key,
              parseNumberOrOff((event.target as HTMLInputElement).value, row.type) ?? 0,
            )}
        ></sl-input>`;
      case 'tags':
        return html`<fmn-tag-field
          class="setting-control"
          label=${row.key}
          placeholder="Add a name"
          .value=${Array.isArray(value) ? value : []}
          @fmn-tags-change=${(event: CustomEvent<TagsChange>) =>
            this.set(row.key, event.detail.value)}
        ></fmn-tag-field>`;
      default:
        // A type this page does not know: shown, not guessed at.
        return html`<code class="setting-control">${settingText(value)}</code>`;
    }
  }

  private renderRow(row: SettingRow): TemplateResult {
    return html`<div class="setting-row">
      <div class="setting-name">
        <code>${row.key}</code>
        <p class="muted">${row.description}</p>
      </div>
      <div class="setting-value">
        ${this.renderControl(row)}
        <span class="setting-type muted">${row.type}</span>
      </div>
    </div>`;
  }

  private renderConflict(doc: SettingsDoc, current: SettingsDoc): TemplateResult {
    const asText = (settings: SettingsDoc): string =>
      settingRows(settings)
        .map((row) => `${row.key}: ${settingText(row.value)}`)
        .join('\n');
    const mine = settingRows(doc)
      .map((row) => `${row.key}: ${settingText(this.valueOf(row))}`)
      .join('\n');
    return html`<section class="conflict">
      <sl-alert variant="warning" open>
        <sl-icon slot="icon" name="exclamation-mark"></sl-icon>
        <strong>Conflict</strong>
        <div class="conflict-versions">
          <span>Loaded version <code>${doc.version ?? 'none'}</code></span>
          <span>Current version <code>${current.version ?? 'none'}</code></span>
        </div>
        <fmn-side-by-side
          leftLabel="Your settings"
          .leftText=${mine}
          rightLabel="Current on server"
          .rightText=${asText(current)}
        ></fmn-side-by-side>
        <sl-button size="small" @click=${() => this.reload()}>Reload current version</sl-button>
      </sl-alert>
    </section>`;
  }

  override render(): TemplateResult {
    return html`
      <header class="page-header">
        <div class="page-name"><sl-icon name="settings"></sl-icon><span>settings</span></div>
        <h1>Settings</h1>
      </header>
      ${gate(this.doc.state, (doc) => {
        const changed = changedKeys(doc, this.edited);
        return html`
          <div class="settings-list">${settingRows(doc).map((row) => this.renderRow(row))}</div>
          ${changed.length === 0
            ? nothing
            : html`<fmn-commit-bar
                .message=${this.message}
                .saving=${this.saving}
                saveLabel="Save"
                label=${`Unsaved settings: ${changed.join(', ')}`}
                @fmn-message-change=${(event: CustomEvent<{ message: string }>) => {
                  this.message = event.detail.message;
                }}
                @fmn-save=${() => void this.save(doc)}
                @fmn-discard=${() => {
                  this.edited = {};
                  this.message = '';
                }}
              ></fmn-commit-bar>`}
          <fmn-validation-errors .errors=${this.errors}></fmn-validation-errors>
          ${this.failure === null
            ? nothing
            : html`<p class="failure" role="alert">${this.failure}</p>`}
          ${this.conflict === null ? nothing : this.renderConflict(doc, this.conflict)}
        `;
      })}
    `;
  }
}

customElements.define('fmn-settings-view', FmnSettingsView);
