/**
 * The body as it is shown under the page title. The server derives the title from the
 * body's first level-1 heading, so that heading is dropped here to show it once; a body
 * that starts with anything else is shown whole.
 */
export function bodyBelowTitle(body: string, title: string): string {
  const lines = body.split('\n');
  const first = lines.findIndex((line) => line.trim() !== '');
  if (first === -1) return body;
  if (lines[first]?.trim() !== `# ${title}`) return body;
  const rest = lines.slice(first + 1);
  while (rest.length > 0 && rest[0]?.trim() === '') rest.shift();
  return rest.join('\n');
}

/** Author name the server records for writes made through this app. */
export const frontendAuthor = 'wiki';
