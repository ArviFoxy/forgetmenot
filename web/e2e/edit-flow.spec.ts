import { expect, test } from '@playwright/test';

// Detects the failures that exist only at this level: the built app cannot complete
// an edit against the real API, and an address loaded directly does not open the
// item it names. Runs against the server at FORGETMENOT_URL.

test.beforeEach(() => {
  test.skip(
    process.env.FORGETMENOT_URL === undefined,
    'FORGETMENOT_URL is unset, so there is no server to run against',
  );
});

test('the built app cannot open a memory from the hierarchy, edit it and show the edit in history', async ({
  page,
}) => {
  const marker = `flow marker ${Date.now()}`;
  const commitMessage = `add ${marker}`;

  await page.goto('/');

  // The hierarchy: filter to the memory, then open it from the sidebar.
  await page.getByLabel('Search scopes and memories').fill('widget-naming');
  const sidebarLink = page.locator('a.memory-link', { hasText: 'widget-naming' }).first();
  await expect(sidebarLink).toBeVisible();
  await sidebarLink.click();
  await expect(page).toHaveURL(/\/memories\/widget-naming$/);
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

  await page.getByRole('link', { name: 'Edit' }).click();
  const editorText = page.locator('.cm-content');
  await expect(editorText).toBeVisible();
  await editorText.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  // One insertion rather than one event per character: typing into a code editor
  // through the browser protocol can reorder single keystrokes, and the subject of
  // this test is the write, not the editor's key handling.
  await page.keyboard.insertText(marker);

  await page.getByLabel('Commit message').fill(commitMessage);
  await page.getByRole('button', { name: 'Save' }).click();

  // A written edit returns to the document, which now carries the new text.
  await expect(page).toHaveURL(/\/memories\/widget-naming$/);
  await expect(page.locator('.markdown').getByText(marker)).toBeVisible();

  await page.getByRole('link', { name: 'History' }).click();
  const commitLink = page.getByRole('link', { name: commitMessage });
  await expect(commitLink).toBeVisible();

  await commitLink.click();
  await expect(page.locator('pre.diff').getByText(`+${marker}`, { exact: false })).toBeVisible();
});

test('a memory id with slashes loses its path when the address is loaded directly', async ({ page }) => {
  await page.goto('/memories/sessions/alpha/session-1/notes');
  await expect(page.getByRole('heading', { level: 1 })).toContainText('bracket rework');
  await expect(page.locator('.infobox')).toContainText('sessions/alpha/session-1/notes');
  // The hierarchy marks the open memory.
  await expect(page.locator('a.memory-link.selected')).toHaveCount(1);
});

test('a scope address with a colon and a slash does not open the scope', async ({ page }) => {
  await page.goto('/scopes/session:alpha/session-1');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('session:alpha/session-1');
  await expect(page.locator('.memory-index').getByRole('link', { name: 'notes' })).toBeVisible();
});

test('a deep link to one commit of a memory opens the memory instead of the commit', async ({
  page,
}) => {
  await page.goto('/memories/widget-naming/history');
  const firstCommit = page.locator('table tbody tr td:first-child a').first();
  await expect(firstCommit).toBeVisible();
  const href = await firstCommit.getAttribute('href');
  expect(href).toMatch(/\/memories\/widget-naming\/history\/[0-9a-f]{40}/);

  await page.goto(href ?? '/');
  await expect(page.locator('pre.diff')).toBeVisible();
  await expect(page.locator('pre.content')).toBeVisible();
});

test('the statistics page reports one figure per memory instead of one per delivery form', async ({
  page,
}) => {
  await page.goto('/stats');
  const header = page.getByRole('columnheader', { name: 'Shown as index line', exact: true });
  await expect(header).toBeVisible();
  await expect(
    page.getByRole('columnheader', { name: 'Shown in full, new', exact: true }),
  ).toBeVisible();
  await expect(page.locator('table.stats').first().locator('tbody tr').first()).toBeVisible();
});
