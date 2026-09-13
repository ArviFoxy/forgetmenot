import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { afterEach, expect, test } from 'vitest';
import { MissingCommitMessage, createApiClient } from '../src/api/client';
import type { MemoryDoc } from '../src/api/types';

type Recorded = { method: string; url: string; body: string };

interface Fixture {
  baseUrl: string;
  requests: Recorded[];
  stop: () => Promise<void>;
}

// A real HTTP server, so the client's response handling is exercised over the wire.
async function serveOnce(
  handler: (request: IncomingMessage, response: ServerResponse, body: string) => void,
): Promise<Fixture> {
  const requests: Recorded[] = [];
  const server: Server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => {
      const body = Buffer.concat(chunks).toString('utf8');
      requests.push({ method: request.method ?? '', url: request.url ?? '', body });
      handler(request, response, body);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('server has no port');
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    requests,
    stop: () => new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
  };
}

const memoryFields: Pick<MemoryDoc, 'description' | 'kind' | 'scopes' | 'source'> = {
  description: 'how widgets are wired',
  kind: 'critical',
  scopes: ['widgets'],
  source: 'user',
};

const serverDoc: MemoryDoc = {
  id: 'widgets',
  name: 'widgets',
  title: 'Widgets',
  ...memoryFields,
  created: '2026-01-02T03:04:05Z',
  modified: '2026-01-02T03:04:05Z',
  author: 'wiki',
  body: '# Widgets\n\nhow widgets are wired\n\nthe version that is on the server\n',
  version: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  links: [],
  backlinks: [],
  last_commit: {
    oid: 'cccccccccccccccccccccccccccccccccccccccc',
    time: '2026-01-02T03:04:05Z',
    author: 'wiki',
    title: 'widen the widget rule',
  },
};

let fixture: Fixture | null = null;

afterEach(async () => {
  if (fixture !== null) {
    await fixture.stop();
    fixture = null;
  }
});

test('a 409 is thrown as a failure instead of returned as a conflict with the server document', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(409, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ current: serverDoc }));
  });
  const client = createApiClient(fixture.baseUrl);

  const outcome = await client.putMemory('widgets', {
    ...memoryFields,
    body: 'the version the editor holds\n',
    base_version: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    author: 'wiki',
    message: 'widen the widget rule',
  });

  expect(outcome.kind).toBe('conflict');
  if (outcome.kind !== 'conflict') return;
  expect(outcome.conflict.current.body).toBe(serverDoc.body);
  expect(outcome.conflict.current.version).toBe(serverDoc.version);
});

test('a 422 is thrown as a failure instead of returned with the validation errors', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(422, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ errors: [{ path: 'memories/widgets.md', message: 'unknown scope id' }] }));
  });
  const client = createApiClient(fixture.baseUrl);

  const outcome = await client.putMemory('widgets', {
    ...memoryFields,
    body: 'body\n',
    base_version: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    author: 'wiki',
    message: 'widen the widget rule',
  });

  expect(outcome.kind).toBe('invalid');
  if (outcome.kind !== 'invalid') return;
  expect(outcome.failure.errors).toEqual([{ path: 'memories/widgets.md', message: 'unknown scope id' }]);
});

test('a 500 is returned as a value instead of throwing with the status and body', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(500, { 'content-type': 'text/plain' });
    response.end('store is locked');
  });
  const client = createApiClient(fixture.baseUrl);

  await expect(client.memory('widgets')).rejects.toThrow(/500/);
  await expect(client.memory('widgets')).rejects.toThrow(/store is locked/);
});

const blankMessages: [string, string][] = [
  ['an empty message', ''],
  ['a message of spaces', '   '],
];

for (const [label, message] of blankMessages) {
  test(`a memory write with ${label} reaches the server`, async () => {
    fixture = await serveOnce((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ commit_oid: 'd'.repeat(40), version: 'e'.repeat(40) }));
    });
    const client = createApiClient(fixture.baseUrl);

    await expect(
      Promise.resolve().then(() =>
        client.putMemory('widgets', {
          ...memoryFields,
          body: 'body\n',
          base_version: 'a'.repeat(40),
          author: 'wiki',
          message,
        }),
      ),
    ).rejects.toThrow(MissingCommitMessage);
    expect(fixture.requests).toEqual([]);
  });
}

test('a memory creation with a blank message reaches the server', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{}');
  });
  const client = createApiClient(fixture.baseUrl);

  await expect(
    Promise.resolve().then(() =>
      client.createMemory({ id: 'widgets', ...memoryFields, body: 'body\n', author: 'wiki', message: '' }),
    ),
  ).rejects.toThrow(MissingCommitMessage);
  expect(fixture.requests).toEqual([]);
});

test('a deletion with a blank message reaches the server', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{}');
  });
  const client = createApiClient(fixture.baseUrl);

  await expect(
    Promise.resolve().then(() =>
      client.deleteMemory('widgets', { base_version: 'a'.repeat(40), author: 'wiki', message: ' ' }),
    ),
  ).rejects.toThrow(MissingCommitMessage);
  expect(fixture.requests).toEqual([]);
});

test('a scope deletion with a blank message reaches the server', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{}');
  });
  const client = createApiClient(fixture.baseUrl);

  await expect(
    Promise.resolve().then(() =>
      client.deleteScope('rocketry', { base_version: 'a'.repeat(40), author: 'wiki', message: '' }),
    ),
  ).rejects.toThrow(MissingCommitMessage);
  expect(fixture.requests).toEqual([]);
});

test('a scope that is still referenced is deleted instead of reporting what refers to it', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(422, { 'content-type': 'application/json' });
    response.end(
      JSON.stringify({ errors: [{ path: 'memories/widget-naming.md', message: 'lists the scope' }] }),
    );
  });
  const client = createApiClient(fixture.baseUrl);

  const outcome = await client.deleteScope('widgets', {
    base_version: 'a'.repeat(40),
    author: 'wiki',
    message: 'remove the widgets scope',
  });

  expect(outcome.kind).toBe('invalid');
  if (outcome.kind !== 'invalid') return;
  expect(outcome.failure.errors[0]?.path).toBe('memories/widget-naming.md');
  expect(fixture.requests[0]?.method).toBe('DELETE');
});

test('a scope write with a blank message reaches the server', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{}');
  });
  const client = createApiClient(fixture.baseUrl);

  await expect(
    Promise.resolve().then(() =>
      client.putScope('rocketry', {
        implies: [],
        triggers: [{ on: 'user_message', pattern: 'rocket' }],
        base_version: 'a'.repeat(40),
        author: 'wiki',
        message: '',
      }),
    ),
  ).rejects.toThrow(MissingCommitMessage);
  expect(fixture.requests).toEqual([]);
});

test('a write with a message is sent without the base version or the message', async () => {
  fixture = await serveOnce((_request, response) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ commit_oid: 'd'.repeat(40), version: 'e'.repeat(40) }));
  });
  const client = createApiClient(fixture.baseUrl);

  const outcome = await client.putMemory('sessions/alpha/s1/notes', {
    ...memoryFields,
    body: 'body\n',
    base_version: 'a'.repeat(40),
    author: 'wiki',
    message: 'widen the widget rule',
  });

  expect(outcome.kind).toBe('written');
  expect(fixture.requests).toHaveLength(1);
  const sent = JSON.parse(fixture.requests[0]?.body ?? '{}') as Record<string, unknown>;
  expect(sent.message).toBe('widen the widget rule');
  expect(sent.base_version).toBe('a'.repeat(40));
  expect(fixture.requests[0]?.url).toBe('/api/memories/sessions/alpha/s1/notes');
});
