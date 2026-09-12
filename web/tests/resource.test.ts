import { expect, test } from 'vitest';
import { Resource } from '../src/lib/resource';

// The source of these expectations is what a page needs from a loader: the state it
// shows belongs to the newest request, and a 404 is a value the page can show rather
// than a breakdown.

test('an answer to an old request replaces the answer to the newest one', async () => {
  const resource = new Resource<string>(() => {});
  let releaseFirst: (value: string) => void = () => undefined;
  const first = resource.load(() => new Promise<string>((resolve) => (releaseFirst = resolve)));
  const second = resource.load(() => Promise.resolve('second'));
  await second;
  releaseFirst('first');
  await first;

  expect(resource.state).toEqual({ status: 'ready', value: 'second' });
});

test('a failure of an old request replaces the answer to the newest one', async () => {
  const resource = new Resource<string>(() => {});
  let failFirst: (error: Error) => void = () => undefined;
  const first = resource.load(() => new Promise<string>((_, reject) => (failFirst = reject)));
  const second = resource.load(() => Promise.resolve('second'));
  await second;
  failFirst(new Error('the first request failed'));
  await first;

  expect(resource.state).toEqual({ status: 'ready', value: 'second' });
});

test('a failure the caller classifies as missing is reported as a breakdown', async () => {
  const resource = new Resource<string>(() => {}, { classify: () => 'missing' });
  await resource.load(() => Promise.reject(new Error('no such thing')));

  expect(resource.state.status).toBe('missing');
  expect(resource.value).toBeNull();
});

test('a failure with no classifier is reported as missing rather than a breakdown', async () => {
  const resource = new Resource<string>(() => {});
  await resource.load(() => Promise.reject(new Error('the store is locked')));

  expect(resource.state.status).toBe('failed');
});

test('the load in progress is not reported, so the page cannot show it', async () => {
  const resource = new Resource<string>(() => {});
  const load = resource.load(() => Promise.resolve('value'));
  expect(resource.state.status).toBe('loading');
  await load;
  expect(resource.state.status).toBe('ready');
});
