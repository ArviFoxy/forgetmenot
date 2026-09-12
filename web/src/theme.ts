// The reader's colour scheme. With no choice stored the page follows the system
// setting, which is what Pico does when the document carries no data-theme.

export type ColourScheme = 'system' | 'light' | 'dark';

const storageKey = 'fmn-colour-scheme';
const schemeChanged = 'fmn-colour-scheme-changed';

export function storedScheme(): ColourScheme {
  const value = window.localStorage.getItem(storageKey);
  return value === 'light' || value === 'dark' ? value : 'system';
}

/** The scheme in force now, with the system setting resolved. */
export function effectiveScheme(scheme: ColourScheme = storedScheme()): 'light' | 'dark' {
  if (scheme !== 'system') return scheme;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function applyScheme(scheme: ColourScheme): void {
  const root = document.documentElement;
  if (scheme === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', scheme);
}

export function setScheme(scheme: ColourScheme): void {
  if (scheme === 'system') window.localStorage.removeItem(storageKey);
  else window.localStorage.setItem(storageKey, scheme);
  applyScheme(scheme);
  window.dispatchEvent(new CustomEvent(schemeChanged));
}

export function onSchemeChange(listener: () => void): () => void {
  const query = window.matchMedia('(prefers-color-scheme: dark)');
  window.addEventListener(schemeChanged, listener);
  query.addEventListener('change', listener);
  return () => {
    window.removeEventListener(schemeChanged, listener);
    query.removeEventListener('change', listener);
  };
}
