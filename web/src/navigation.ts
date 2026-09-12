// Browser navigation: the address bar, the back button, and clicks on in-app links.
// Matching an address to a view is routes.ts; this module only moves between them.

import { isAppUrl } from './routes';

const locationChanged = 'fmn-location-changed';
const storeChanged = 'fmn-store-changed';

export function currentPath(): string {
  return window.location.pathname;
}

/** Go to an in-app address, as a link click or a finished write would. */
export function navigate(path: string, options: { replace?: boolean } = {}): void {
  if (options.replace === true) window.history.replaceState(null, '', path);
  else window.history.pushState(null, '', path);
  window.dispatchEvent(new CustomEvent(locationChanged));
}

export function onLocationChange(listener: () => void): () => void {
  window.addEventListener(locationChanged, listener);
  window.addEventListener('popstate', listener);
  return () => {
    window.removeEventListener(locationChanged, listener);
    window.removeEventListener('popstate', listener);
  };
}

/** The store was written to, so anything showing store contents is out of date. */
export function announceStoreChange(): void {
  window.dispatchEvent(new CustomEvent(storeChanged));
}

export function onStoreChange(listener: () => void): () => void {
  window.addEventListener(storeChanged, listener);
  return () => window.removeEventListener(storeChanged, listener);
}

/**
 * Turns clicks on in-app links into navigation. A modified click, a new tab, a
 * download and an outside address are left to the browser.
 */
export function interceptLinkClicks(): void {
  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const anchor = target.closest('a');
    if (anchor === null || anchor.target === '_blank' || anchor.hasAttribute('download')) return;
    const href = anchor.getAttribute('href');
    if (href === null || href.startsWith('#')) return;
    const url = new URL(href, window.location.href);
    if (!isAppUrl(url)) return;
    event.preventDefault();
    navigate(`${url.pathname}${url.search}`);
  });
}
