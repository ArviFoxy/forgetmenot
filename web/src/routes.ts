// The one place app URLs are defined. Every link in the app is built by `paths`,
// and every address the browser can be at is matched by `resolve`, so a link and
// the view it opens cannot drift apart.

import UniversalRouterSync from 'universal-router/sync';
import type { RouteParams } from 'universal-router';

export type RouteName =
  | 'home'
  | 'memory'
  | 'memoryEdit'
  | 'memoryHistory'
  | 'memoryCommit'
  | 'memoryNew'
  | 'scope'
  | 'scopeEdit'
  | 'contexts'
  | 'review'
  | 'stats'
  | 'triggerTest'
  | 'unknown';

/** The element that shows an address, and the properties it is given. */
export interface RouteView {
  name: RouteName;
  tag: string;
  properties: Record<string, string>;
}

/**
 * One path segment per id segment: a memory id may contain slashes and a session
 * scope id contains both a colon and a slash. A colon is left readable because it
 * is legal in a path segment.
 */
function encodeId(id: string): string {
  return id
    .split('/')
    .map((segment) => encodeURIComponent(segment).replaceAll('%3A', ':'))
    .join('/');
}

/** Every link the app shows. No component builds a path of its own. */
export const paths = {
  home: (): string => '/',
  memory: (id: string): string => `/memories/${encodeId(id)}`,
  memoryEdit: (id: string): string => `/memories/${encodeId(id)}/edit`,
  memoryHistory: (id: string): string => `/memories/${encodeId(id)}/history`,
  memoryCommit: (id: string, oid: string): string =>
    `/memories/${encodeId(id)}/history/${encodeURIComponent(oid)}`,
  memoryNew: (): string => '/memories/new',
  scope: (id: string): string => `/scopes/${encodeId(id)}`,
  scopeEdit: (id: string): string => `/scopes/${encodeId(id)}/edit`,
  contexts: (): string => '/contexts',
  review: (): string => '/review',
  stats: (): string => '/stats',
  triggerTest: (): string => '/triggers/test',
};

/** The id a wildcard matched, with its slashes back in place. */
function joined(params: RouteParams, key: string): string {
  const value = params[key];
  if (Array.isArray(value)) return value.join('/');
  return value ?? '';
}

function view(name: RouteName, tag: string, properties: Record<string, string> = {}): RouteView {
  return { name, tag, properties };
}

// The suffix routes come before the bare id, so /memories/<id>/history is the
// history of <id> rather than a memory whose last segment is "history".
const router = new UniversalRouterSync<RouteView>([
  { path: '/', action: () => view('home', 'fmn-overview-view') },
  { path: '/memories/new', action: () => view('memoryNew', 'fmn-memory-new') },
  {
    path: '/memories/*id/history/:oid',
    action: ({ params }) =>
      view('memoryCommit', 'fmn-memory-page', {
        memoryId: joined(params, 'id'),
        mode: 'commit',
        oid: joined(params, 'oid'),
      }),
  },
  {
    path: '/memories/*id/history',
    action: ({ params }) =>
      view('memoryHistory', 'fmn-memory-page', { memoryId: joined(params, 'id'), mode: 'history' }),
  },
  {
    path: '/memories/*id/edit',
    action: ({ params }) =>
      view('memoryEdit', 'fmn-memory-page', { memoryId: joined(params, 'id'), mode: 'edit' }),
  },
  {
    path: '/memories/*id',
    action: ({ params }) =>
      view('memory', 'fmn-memory-page', { memoryId: joined(params, 'id'), mode: 'document' }),
  },
  {
    path: '/scopes/*id/edit',
    action: ({ params }) =>
      view('scopeEdit', 'fmn-scope-page', { scopeId: joined(params, 'id'), mode: 'edit' }),
  },
  {
    path: '/scopes/*id',
    action: ({ params }) =>
      view('scope', 'fmn-scope-page', { scopeId: joined(params, 'id'), mode: 'view' }),
  },
  { path: '/contexts', action: () => view('contexts', 'fmn-contexts-view') },
  { path: '/review', action: () => view('review', 'fmn-review-view') },
  { path: '/stats', action: () => view('stats', 'fmn-stats-view') },
  { path: '/triggers/test', action: () => view('triggerTest', 'fmn-trigger-test-view') },
  { path: '/*rest', action: () => view('unknown', 'fmn-unknown-view') },
]);

/** The view for an address, loaded directly or reached by a link. */
export function resolve(pathname: string): RouteView {
  const match = router.resolve(pathname);
  return match ?? view('unknown', 'fmn-unknown-view');
}

/** True when an address belongs to this app, so a click on it is navigation. */
export function isAppUrl(url: URL): boolean {
  return url.origin === window.location.origin && !url.pathname.startsWith('/api/');
}
