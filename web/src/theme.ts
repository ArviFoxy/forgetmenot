// The reader's colour scheme. Shoelace's dark theme applies under the
// `sl-theme-dark` class, so the choice is resolved here and written to the document
// element; with no choice stored the system setting decides.

export type ColourScheme = 'system' | 'light' | 'dark';

const storageKey = 'fmn-colour-scheme';
const darkClass = 'sl-theme-dark';
const schemeChanged = 'fmn-colour-scheme-changed';

function darkQuery(): MediaQueryList {
  return window.matchMedia('(prefers-color-scheme: dark)');
}

export function storedScheme(): ColourScheme {
  const value = window.localStorage.getItem(storageKey);
  return value === 'light' || value === 'dark' ? value : 'system';
}

/** The scheme in force now, with the system setting resolved. */
export function effectiveScheme(scheme: ColourScheme = storedScheme()): 'light' | 'dark' {
  if (scheme !== 'system') return scheme;
  return darkQuery().matches ? 'dark' : 'light';
}

export function applyScheme(scheme: ColourScheme): void {
  document.documentElement.classList.toggle(darkClass, effectiveScheme(scheme) === 'dark');
}

export function setScheme(scheme: ColourScheme): void {
  if (scheme === 'system') window.localStorage.removeItem(storageKey);
  else window.localStorage.setItem(storageKey, scheme);
  applyScheme(scheme);
  window.dispatchEvent(new CustomEvent(schemeChanged));
}

export function onSchemeChange(listener: () => void): () => void {
  const query = darkQuery();
  window.addEventListener(schemeChanged, listener);
  query.addEventListener('change', listener);
  return () => {
    window.removeEventListener(schemeChanged, listener);
    query.removeEventListener('change', listener);
  };
}

/** Keeps the document in step with the system setting while the choice is `system`. */
export function watchScheme(): void {
  darkQuery().addEventListener('change', () => applyScheme(storedScheme()));
}
