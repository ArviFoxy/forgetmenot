// Resource types of the server's JSON API, mirrored from the project's wire types.
// Field names are the JSON field names and stay in snake_case.

export type MemoryKind = 'critical' | 'knowledge';
export type MemorySource = 'user' | 'assistant';
export type ScopeType = 'global' | 'machine' | 'session' | 'project' | 'domain' | 'directory';
export type TriggerField =
  | 'user_message'
  | 'assistant_message'
  | 'tool_name'
  | 'tool_input'
  | 'tool_result'
  | 'working_directory';
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
  archived: boolean;
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
  created: string | null;
  modified: string | null;
  author: string | null;
  archived: boolean;
  body: string;
  version: string;
  links: string[];
  backlinks: string[];
  last_commit: Commit | null;
}

export interface Trigger {
  on: TriggerField;
  pattern: string;
  machine?: string;
}

export interface ScopeDoc {
  id: string;
  type: ScopeType;
  implies: string[];
  triggers: Trigger[];
  version: string;
}

export interface HistoryEntry {
  commit: Commit;
  content: string;
  diff: string;
}

export interface ContextRow {
  key: string;
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
  archived?: boolean;
}

export interface WriteRequest {
  description: string;
  kind: MemoryKind;
  scopes: string[];
  source: MemorySource;
  body: string;
  base_version?: string;
  author: string;
  message: string;
}

/** Creation needs the id of the new memory, which is its path under memories/. */
export type MemoryCreateRequest = WriteRequest & { id: string };

export interface ArchiveRequest {
  base_version: string;
  author: string;
  message: string;
}

export interface ScopeWriteRequest {
  type: ScopeType;
  implies: string[];
  triggers: Trigger[];
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

export interface Conflict<Doc = MemoryDoc> {
  current: Doc;
}

export interface ValidationFailure {
  errors: ValidationError[];
}

export type WriteOutcome<Doc = MemoryDoc> =
  | { kind: 'written'; response: WriteResponse }
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
