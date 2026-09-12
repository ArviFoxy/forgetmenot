import type {
  ArchiveRequest,
  Commit,
  ContextRow,
  Conflict,
  DenyDayRow,
  HistoryEntry,
  LatencyRow,
  MemoryCreateRequest,
  MemoryDoc,
  MemoryIndexFilter,
  MemoryStatsRow,
  MemorySummary,
  ReviewReport,
  ScopeCreateRequest,
  ScopeDoc,
  ScopeStatsRow,
  ScopeWriteRequest,
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

function memoryPath(id: string): string {
  return id.split('/').map(encodeURIComponent).join('/');
}

function indexQuery(filter: MemoryIndexFilter | undefined): string {
  if (!filter) return '';
  const query = new URLSearchParams();
  if (filter.scope) query.set('scope', filter.scope);
  if (filter.kind) query.set('kind', filter.kind);
  if (filter.archived !== undefined) query.set('archived', String(filter.archived));
  const text = query.toString();
  return text === '' ? '' : `?${text}`;
}

export interface ApiClient {
  memoryIndex(filter?: MemoryIndexFilter): Promise<MemorySummary[]>;
  memory(id: string): Promise<MemoryDoc>;
  putMemory(id: string, request: WriteRequest): Promise<WriteOutcome<MemoryDoc>>;
  createMemory(request: MemoryCreateRequest): Promise<WriteOutcome<MemoryDoc>>;
  archiveMemory(id: string, request: ArchiveRequest): Promise<WriteOutcome<MemoryDoc>>;
  memoryHistory(id: string): Promise<Commit[]>;
  memoryHistoryEntry(id: string, oid: string): Promise<HistoryEntry>;
  scopeIndex(): Promise<ScopeDoc[]>;
  scope(id: string): Promise<ScopeDoc>;
  putScope(id: string, request: ScopeWriteRequest): Promise<WriteOutcome<ScopeDoc>>;
  createScope(request: ScopeCreateRequest): Promise<WriteOutcome<ScopeDoc>>;
  scopeHistory(id: string): Promise<Commit[]>;
  testTriggers(request: TriggerTestRequest): Promise<TriggerTestResult>;
  contexts(): Promise<ContextRow[]>;
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
  async function write<Doc>(
    method: 'PUT' | 'POST',
    path: string,
    body: unknown,
  ): Promise<WriteOutcome<Doc>> {
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
    return { kind: 'written', response: (await response.json()) as WriteResponse };
  }

  return {
    memoryIndex: (filter) => read<MemorySummary[]>(`/api/memories${indexQuery(filter)}`),
    memory: (id) => read<MemoryDoc>(`/api/memories/${memoryPath(id)}`),

    putMemory(id, request) {
      requireMessage(`memory ${id}`, request.message);
      return write<MemoryDoc>('PUT', `/api/memories/${memoryPath(id)}`, request);
    },
    createMemory(request) {
      requireMessage(`memory ${request.id}`, request.message);
      return write<MemoryDoc>('POST', '/api/memories', request);
    },
    archiveMemory(id, request) {
      requireMessage(`archive of memory ${id}`, request.message);
      return write<MemoryDoc>('POST', `/api/memories/${memoryPath(id)}/archive`, request);
    },

    memoryHistory: (id) => read<Commit[]>(`/api/memories/${memoryPath(id)}/history`),
    memoryHistoryEntry: (id, oid) =>
      read<HistoryEntry>(`/api/memories/${memoryPath(id)}/history/${encodeURIComponent(oid)}`),

    scopeIndex: () => read<ScopeDoc[]>('/api/scopes'),
    scope: (id) => read<ScopeDoc>(`/api/scopes/${encodeURIComponent(id)}`),

    putScope(id, request) {
      requireMessage(`scope ${id}`, request.message);
      return write<ScopeDoc>('PUT', `/api/scopes/${encodeURIComponent(id)}`, request);
    },
    createScope(request) {
      requireMessage(`scope ${request.id}`, request.message);
      return write<ScopeDoc>('POST', '/api/scopes', request);
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

    contexts: () => read<ContextRow[]>('/api/contexts'),
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
