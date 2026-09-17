// The one place app URLs are defined. Every link in the app is built by `paths`,
// and every address the browser can be at is matched by `resolve`, so a link and
// the view it opens cannot drift apart.

import UniversalRouterSync from 'universal-router/sync';
import type { RouteParams } from 'universal-router';
import type { PromptMode } from './api/types';

export type RouteName =
  | 'home'
  | 'memory'
  | 'memoryHistory'
  | 'memoryCommit'
  | 'memoryNew'
  | 'scope'
  | 'scopeNew'
  | 'contexts'
  | 'contextPrompt'
  | 'stats'
  | 'settings'
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
  memoryHistory: (id: string): string => `/memories/${encodeId(id)}/history`,
  memoryCommit: (id: string, oid: string): string =>
    `/memories/${encodeId(id)}/history/${encodeURIComponent(oid)}`,
  /** With a scope, the new memory starts out in it. */
  memoryNew: (scope?: string): string =>
    scope === undefined || scope === ''
      ? '/memories/new'
      : `/memories/new?scope=${encodeURIComponent(scope)}`,
  scopeNew: (): string => '/scopes/new',
  scope: (id: string): string => `/scopes/${encodeId(id)}`,
  contexts: (): string => '/contexts',
  /** The text a context would be given: what is due, or the whole of its scopes. */
  contextPrompt: (key: string, mode: PromptMode = 'due'): string =>
    mode === 'due'
      ? `/contexts/${encodeId(key)}/prompt`
      : `/contexts/${encodeId(key)}/prompt?mode=${mode}`,
  stats: (): string => '/stats',
  settings: (): string => '/settings',
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
// history of <id> rather than a memory whose last segment is "history". There is no
// edit route: an item's own page is where it is edited.
const router = new UniversalRouterSync<RouteView>([
  { path: '/', action: () => view('home', 'fmn-overview-view') },
  // The same views as an item that exists, in the state of one that does not: the
  // fields and the commit bar are the item's own, starting from nothing.
  { path: '/memories/new', action: () => view('memoryNew', 'fmn-memory-page', { mode: 'new' }) },
  { path: '/scopes/new', action: () => view('scopeNew', 'fmn-scope-page', { mode: 'new' }) },
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
    path: '/memories/*id',
    action: ({ params }) =>
      view('memory', 'fmn-memory-page', { memoryId: joined(params, 'id'), mode: 'document' }),
  },
  {
    path: '/scopes/*id',
    action: ({ params }) => view('scope', 'fmn-scope-page', { scopeId: joined(params, 'id') }),
  },
  {
    path: '/contexts/*key/prompt',
    action: ({ params }) =>
      view('contextPrompt', 'fmn-context-prompt-view', { contextKey: joined(params, 'key') }),
  },
  { path: '/contexts', action: () => view('contexts', 'fmn-contexts-view') },
  { path: '/stats', action: () => view('stats', 'fmn-stats-view') },
  { path: '/settings', action: () => view('settings', 'fmn-settings-view') },
  { path: '/*rest', action: () => view('unknown', 'fmn-unknown-view') },
]);

/** The view for an address, loaded directly or reached by a link. */
export function resolve(pathname: string): RouteView {
  const match = router.resolve(pathname);
  return match ?? view('unknown', 'fmn-unknown-view');
}

/** The mode a prompt address asks for, as `paths.contextPrompt` wrote it. */
export function promptModeFromSearch(search: string = window.location.search): PromptMode {
  return new URLSearchParams(search).get('mode') === 'all' ? 'all' : 'due';
}

/** The scope a new memory starts in, as `paths.memoryNew` wrote it. */
export function scopeFromSearch(search: string = window.location.search): string {
  return new URLSearchParams(search).get('scope') ?? '';
}

/** True when an address belongs to this app, so a click on it is navigation. */
export function isAppUrl(url: URL): boolean {
  return url.origin === window.location.origin && !url.pathname.startsWith('/api/');
}
