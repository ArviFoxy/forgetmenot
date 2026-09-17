// Resource types of the server's JSON API, mirrored from the project's wire types.
// Field names are the JSON field names and stay in snake_case.

export type MemoryKind = 'critical' | 'knowledge';
/**
 * Who wrote the memory. `user` and `assistant` are the conventional values; any
 * other string a person or another tool keeps in the file is stored and read back
 * as it was written.
 */
export type MemorySource = string;
/**
 * Which of the hook's texts a trigger matches. `any` is not one of the texts: it
 * stands for every one of them, and is what a trigger without an `on` line means.
 */
export type TriggerField =
  | 'any'
  | 'user_message'
  | 'assistant_message'
  | 'tool_name'
  | 'tool_input'
  | 'tool_result'
  | 'shell_directory'
  | 'session_directory';
export type DeliveryReason = 'new' | 'changed' | 'stale' | 'retracted';

export interface MemorySummary {
  id: string;
  name: string;
  title: string;
  description: string;
  kind: MemoryKind;
  scopes: string[];
  source: MemorySource;
  modified: string | null;
  version: string;
}

export interface Commit {
  oid: string;
  time: string;
  author: string;
  title: string;
}

// `title` is the body's first level-1 heading, else the name; the server derives it and
// it is read-only here.
export interface MemoryDoc {
  id: string;
  name: string;
  title: string;
  description: string;
  kind: MemoryKind;
  scopes: string[];
  source: MemorySource;
  /** The `metadata` keys forgetmenot does not interpret, Claude Code's own `type` among them. */
  metadata: Record<string, unknown>;
  created: string | null;
  modified: string | null;
  author: string | null;
  body: string;
  version: string;
  links: string[];
  backlinks: string[];
  last_commit: Commit | null;
}

export interface Trigger {
  /** Absent means `any`: a file that says nothing about `on` is written back the same. */
  on?: TriggerField;
  pattern: string;
  /** The trigger fires only on this machine; absent means every machine. */
  machine?: string;
}

/** Whether a pattern compiles, with the regex engine's own message when it does not. */
export interface PatternValidity {
  ok: boolean;
  error?: string;
}

export interface MachineList {
  machines: string[];
}

/** The types the settings schema uses, spelled as the server spells them. */
export type SettingType = 'integer or null' | 'integer' | 'bool' | 'list of strings';

export type SettingValue = number | boolean | string[] | null;

export interface SettingSchemaRow {
  key: string;
  type: SettingType;
  /** The value in force when the store's file sets nothing. */
  default: SettingValue;
  description: string;
}

/**
 * Every setting with the value in force, the version of the file they were read
 * at, and what each key means. The version is null when the store has no file
 * yet, which the first write sends back as no base version at all.
 */
export interface SettingsDoc {
  settings: Record<string, SettingValue>;
  version: string | null;
  schema: SettingSchemaRow[];
}

/** A change to one setting, which is one commit. */
export interface SettingsWriteRequest {
  value: SettingValue;
  base_version?: string;
  author: string;
  message: string;
}

/** When a scope turns itself off in a context. */
export interface Forget {
  /** Context tokens since the scope was last activated. */
  tokens_since_trigger: number;
}

export interface ScopeDoc {
  id: string;
  /** The text the scope delivers whenever it is active; null when it delivers none. */
  message: string | null;
  implies: string[];
  triggers: Trigger[];
  /** When the scope turns itself off; null when it stays on until the agent turns it off. */
  forget: Forget | null;
  version: string;
}

/** What a scope id names, decided by the server. */
export type ScopeKind = 'global' | 'machine' | 'session' | 'file';

/** One scope that exists, as the scope index lists it. */
export interface ScopeRow {
  id: string;
  kind: ScopeKind;
  /** What to call a session, from its context; null for the other kinds and for a session with no live context. */
  name: string | null;
  /** The scope's file, for the kind `file`; null for the other kinds. */
  file: ScopeDoc | null;
}

export interface HistoryEntry {
  commit: Commit;
  content: string;
  diff: string;
}

export interface ContextRow {
  key: string;
  /** What to call the context: its task, its title, or its first prompt. */
  name: string;
  /** The name the user gave the session with `/rename`. */
  title: string | null;
  first_prompt: string | null;
  /** The key of the session a subagent runs in; null for a session itself. */
  parent: string | null;
  /** The task a subagent was given; null for a session itself. */
  task: string | null;
  /** The kind of subagent, such as `general-purpose`; null for a session. */
  agent_type: string | null;
  active_scopes: string[];
  delivered_count: number;
  last_seen: string;
}

export interface ValidationError {
  path: string;
  message: string;
}

export interface ReviewReport {
  errors: ValidationError[];
  global_only_critical: string[];
}

/**
 * What was delivered for one memory. Index lines and full bodies are separate
 * counts and are never added together: they cost different amounts of context.
 */
export interface MemoryStatsRow {
  memory: string;
  shown_index: number;
  shown_full_new: number;
  shown_full_changed: number;
  shown_full_stale: number;
  /** Times the model fetched the whole body itself, through the tool. */
  fetched_full: number;
  retracted: number;
  last_shown: string | null;
}

export interface TriggerStatsRow {
  scope_id: string;
  field: TriggerField;
  pattern: string;
  fires: number;
  new_activations: number;
  /** The fraction of the fires whose event stopped a tool call. */
  deny_share: number;
}

export interface ScopeStatsRow {
  scope_id: string;
  activations: number;
  /** Times the scope turned itself off because its forget rule was reached. */
  forgettings: number;
  live_contexts: number;
}

export interface DenyDayRow {
  /** The day in UTC, as YYYY-MM-DD. */
  day: string;
  denies: number;
  events: number;
}

/** Hook latency for the events of one name, in microseconds. */
export interface LatencyRow {
  event: string;
  count: number;
  p50_us: number;
  p90_us: number;
  p99_us: number;
  max_us: number;
}

/** Delivered bytes for one session, with the two forms apart. */
export interface SessionBytesRow {
  session_key: string;
  bytes_full: number;
  bytes_index: number;
}

export interface MemoryIndexFilter {
  scope?: string;
  kind?: MemoryKind;
}

export interface WriteRequest {
  description: string;
  kind: MemoryKind;
  scopes: string[];
  source: MemorySource;
  /**
   * The `metadata` keys forgetmenot does not interpret, replacing the ones the memory
   * carries. Absent leaves them as they are.
   */
  metadata?: Record<string, unknown>;
  body: string;
  base_version?: string;
  author: string;
  message: string;
}

/** Creation needs the id of the new memory, which is its path under memories/. */
export type MemoryCreateRequest = WriteRequest & { id: string };

/** A memory is deleted rather than archived: the git history is the archive. */
export interface DeleteRequest {
  base_version: string;
  author: string;
  message: string;
}

export interface ScopeWriteRequest {
  implies: string[];
  triggers: Trigger[];
  /** The scope's own message, apart from `message`, which is the commit title. */
  scope_message: string | null;
  /** When the scope turns itself off; null when it stays on until the agent turns it off. */
  forget: Forget | null;
  base_version?: string;
  author: string;
  message: string;
}

/** Creation needs the id of the new scope. */
export type ScopeCreateRequest = ScopeWriteRequest & { id: string };

export interface WriteResponse {
  commit_oid: string;
  version: string;
}

/** A delete answers with the commit that removed the file, and nothing else. */
export interface DeleteResponse {
  commit_oid: string;
}

export interface Conflict<Doc = MemoryDoc> {
  current: Doc;
}

export interface ValidationFailure {
  errors: ValidationError[];
}

export type WriteOutcome<Doc = MemoryDoc, Written = WriteResponse> =
  | { kind: 'written'; response: Written }
  | { kind: 'conflict'; conflict: Conflict<Doc> }
  | { kind: 'invalid'; failure: ValidationFailure };

export interface TriggerTestRequest {
  field: TriggerField;
  text: string;
  machine: string;
}

export interface TriggerMatch {
  scope_id: string;
  field: TriggerField;
  pattern: string;
}

export interface TriggerTestResult {
  fired: TriggerMatch[];
}
