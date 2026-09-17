import type {
  Commit,
  ContextPrompt,
  ContextRow,
  Conflict,
  DeleteRequest,
  DeleteResponse,
  DenyDayRow,
  HistoryEntry,
  LatencyRow,
  MachineList,
  MemoryCreateRequest,
  MemoryDoc,
  MemoryIndexFilter,
  MemoryStatsRow,
  MemorySummary,
  PatternValidity,
  PromptMode,
  ReviewReport,
  ScopeCreateRequest,
  ScopeDoc,
  ScopeRow,
  ScopeStatsRow,
  ScopeWriteRequest,
  SettingsDoc,
  SettingsWriteRequest,
  SessionBytesRow,
  TriggerStatsRow,
  TriggerTestRequest,
  TriggerTestResult,
  ValidationFailure,
  WriteOutcome,
  WriteRequest,
  WriteResponse,
} from './types';

/** A non-2xx response the caller cannot act on, with the server's status and body. */
export class RequestFailed extends Error {
  constructor(
    readonly method: string,
    readonly url: string,
    readonly status: number,
    readonly bodyText: string,
  ) {
    super(`${method} ${url} failed with ${status}: ${bodyText}`);
    this.name = 'RequestFailed';
  }
}

/** A write was attempted without a commit message; no request is sent. */
export class MissingCommitMessage extends Error {
  constructor(readonly target: string) {
    super(`write to ${target} needs a commit message`);
    this.name = 'MissingCommitMessage';
  }
}

function requireMessage(target: string, message: unknown): void {
  if (typeof message !== 'string' || message.trim() === '') {
    throw new MissingCommitMessage(target);
  }
}

/** An id or a key as path segments: the slashes stay, everything else is escaped. */
function encodedPath(id: string): string {
  return id.split('/').map(encodeURIComponent).join('/');
}

function indexQuery(filter: MemoryIndexFilter | undefined): string {
  if (!filter) return '';
  const query = new URLSearchParams();
  if (filter.scope) query.set('scope', filter.scope);
  if (filter.kind) query.set('kind', filter.kind);
  const text = query.toString();
  return text === '' ? '' : `?${text}`;
}

export interface ApiClient {
  memoryIndex(filter?: MemoryIndexFilter): Promise<MemorySummary[]>;
  memory(id: string): Promise<MemoryDoc>;
  putMemory(id: string, request: WriteRequest): Promise<WriteOutcome<MemoryDoc>>;
  createMemory(request: MemoryCreateRequest): Promise<WriteOutcome<MemoryDoc>>;
  deleteMemory(id: string, request: DeleteRequest): Promise<WriteOutcome<MemoryDoc, DeleteResponse>>;
  memoryHistory(id: string): Promise<Commit[]>;
  memoryHistoryEntry(id: string, oid: string): Promise<HistoryEntry>;
  scopeIndex(): Promise<ScopeRow[]>;
  scope(id: string): Promise<ScopeDoc>;
  putScope(id: string, request: ScopeWriteRequest): Promise<WriteOutcome<ScopeDoc>>;
  createScope(request: ScopeCreateRequest): Promise<WriteOutcome<ScopeDoc>>;
  deleteScope(id: string, request: DeleteRequest): Promise<WriteOutcome<ScopeDoc, DeleteResponse>>;
  scopeHistory(id: string): Promise<Commit[]>;
  testTriggers(request: TriggerTestRequest): Promise<TriggerTestResult>;
  /** Whether one pattern is a regex the store would accept. */
  validatePattern(pattern: string): Promise<PatternValidity>;
  /** Every machine the server has heard from, for the fields that name one. */
  machineIndex(): Promise<string[]>;
  settings(): Promise<SettingsDoc>;
  /** One setting, written as one commit; a stale version comes back as a conflict. */
  putSetting(key: string, request: SettingsWriteRequest): Promise<WriteOutcome<SettingsDoc>>;
  contexts(): Promise<ContextRow[]>;
  /** The text one context would be given next, or the whole of its scopes. */
  contextPrompt(key: string, mode: PromptMode): Promise<ContextPrompt>;
  review(): Promise<ReviewReport>;
  memoryStats(): Promise<MemoryStatsRow[]>;
  triggerStats(): Promise<TriggerStatsRow[]>;
  scopeStats(): Promise<ScopeStatsRow[]>;
  denyStats(): Promise<DenyDayRow[]>;
  latencyStats(): Promise<LatencyRow[]>;
  sessionStats(): Promise<SessionBytesRow[]>;
}

export function createApiClient(baseUrl = '', fetchImpl: typeof fetch = fetch): ApiClient {
  const root = baseUrl.replace(/\/$/, '');

  async function read<Resource>(path: string): Promise<Resource> {
    const url = `${root}${path}`;
    const response = await fetchImpl(url, { headers: { accept: 'application/json' } });
    if (!response.ok) {
      throw new RequestFailed('GET', url, response.status, await response.text());
    }
    return (await response.json()) as Resource;
  }

  // 409 carries the current document, 422 the validation errors; both are values the
  // page shows, not failures. Anything else is a failure the page cannot act on.
  async function write<Doc, Written = WriteResponse>(
    method: 'PUT' | 'POST' | 'DELETE',
    path: string,
    body: unknown,
  ): Promise<WriteOutcome<Doc, Written>> {
    const url = `${root}${path}`;
    const response = await fetchImpl(url, {
      method,
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify(body),
    });
    if (response.status === 409) {
      const payload = (await response.json()) as Conflict<Doc> | Doc;
      const current =
        payload !== null && typeof payload === 'object' && 'current' in payload
          ? (payload as Conflict<Doc>).current
          : (payload as Doc);
      return { kind: 'conflict', conflict: { current } };
    }
    if (response.status === 422) {
      const payload = (await response.json()) as Partial<ValidationFailure>;
      return { kind: 'invalid', failure: { errors: payload.errors ?? [] } };
    }
    if (!response.ok) {
      throw new RequestFailed(method, url, response.status, await response.text());
    }
    return { kind: 'written', response: (await response.json()) as Written };
  }

  return {
    memoryIndex: (filter) => read<MemorySummary[]>(`/api/memories${indexQuery(filter)}`),
    memory: (id) => read<MemoryDoc>(`/api/memories/${encodedPath(id)}`),

    putMemory(id, request) {
      requireMessage(`memory ${id}`, request.message);
      return write<MemoryDoc>('PUT', `/api/memories/${encodedPath(id)}`, request);
    },
    createMemory(request) {
      requireMessage(`memory ${request.id}`, request.message);
      return write<MemoryDoc>('POST', '/api/memories', request);
    },
    deleteMemory(id, request) {
      requireMessage(`deletion of memory ${id}`, request.message);
      return write<MemoryDoc, DeleteResponse>('DELETE', `/api/memories/${encodedPath(id)}`, request);
    },

    memoryHistory: (id) => read<Commit[]>(`/api/memories/${encodedPath(id)}/history`),
    memoryHistoryEntry: (id, oid) =>
      read<HistoryEntry>(`/api/memories/${encodedPath(id)}/history/${encodeURIComponent(oid)}`),

    scopeIndex: () => read<ScopeRow[]>('/api/scopes'),
    scope: (id) => read<ScopeDoc>(`/api/scopes/${encodeURIComponent(id)}`),

    putScope(id, request) {
      requireMessage(`scope ${id}`, request.message);
      return write<ScopeDoc>('PUT', `/api/scopes/${encodeURIComponent(id)}`, request);
    },
    createScope(request) {
      requireMessage(`scope ${request.id}`, request.message);
      return write<ScopeDoc>('POST', '/api/scopes', request);
    },

    deleteScope(id, request) {
      requireMessage(`deletion of scope ${id}`, request.message);
      return write<ScopeDoc, DeleteResponse>('DELETE', `/api/scopes/${encodeURIComponent(id)}`, request);
    },

    scopeHistory: (id) => read<Commit[]>(`/api/scopes/${encodeURIComponent(id)}/history`),

    async testTriggers(request) {
      const url = `${root}/api/triggers/test`;
      const response = await fetchImpl(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json', accept: 'application/json' },
        body: JSON.stringify(request),
      });
      if (!response.ok) {
        throw new RequestFailed('POST', url, response.status, await response.text());
      }
      return (await response.json()) as TriggerTestResult;
    },

    async validatePattern(pattern) {
      const url = `${root}/api/triggers/validate`;
      const response = await fetchImpl(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json', accept: 'application/json' },
        body: JSON.stringify({ pattern }),
      });
      if (!response.ok) {
        throw new RequestFailed('POST', url, response.status, await response.text());
      }
      return (await response.json()) as PatternValidity;
    },

    machineIndex: async () => (await read<MachineList>('/api/machines')).machines,

    settings: () => read<SettingsDoc>('/api/settings'),

    putSetting(key, request) {
      requireMessage(`the setting ${key}`, request.message);
      return write<SettingsDoc>('PUT', `/api/settings/${encodeURIComponent(key)}`, request);
    },

    contexts: () => read<ContextRow[]>('/api/contexts'),
    contextPrompt: (key, mode) =>
      read<ContextPrompt>(`/api/contexts/${encodedPath(key)}/prompt?mode=${mode}`),
    review: () => read<ReviewReport>('/api/review'),
    memoryStats: () => read<MemoryStatsRow[]>('/api/stats/memories'),
    triggerStats: () => read<TriggerStatsRow[]>('/api/stats/triggers'),
    scopeStats: () => read<ScopeStatsRow[]>('/api/stats/scopes'),
    denyStats: () => read<DenyDayRow[]>('/api/stats/denies'),
    latencyStats: () => read<LatencyRow[]>('/api/stats/latency'),
    sessionStats: () => read<SessionBytesRow[]>('/api/stats/sessions'),
  };
}

/** The client the pages use: same origin as the app. */
export const api = createApiClient();
