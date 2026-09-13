import { expect, test, type Page } from '@playwright/test';

// Detects the failures that exist only at this level: the built app cannot complete
// an edit against the real API, an address loaded directly does not open the item it
// names, and the editor's round trip damages the stored text. The server and its
// store come from the global setup, so these writes touch a copy of the example
// store and nothing else.

async function openMemory(page: Page, id: string): Promise<void> {
  await page.goto(`/memories/${id}`);
  await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
}

/** Puts the caret at the end of the document and types there. */
async function typeAtEnd(page: Page, text: string): Promise<void> {
  const editor = page.locator('.milkdown .ProseMirror');
  await editor.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.insertText(text);
}

async function save(page: Page, message: string): Promise<void> {
  const bar = page.locator('.commit-bar');
  await expect(bar).toBeVisible();
  await bar.locator('input').fill(message);
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(bar).toBeHidden();
}

async function storedBody(page: Page, id: string): Promise<string> {
  const response = await page.request.get(`/api/memories/${id}`);
  const doc = (await response.json()) as { body: string };
  return doc.body;
}

test('the built app cannot open a memory from the tree, edit it and show the edit in history', async ({
  page,
}) => {
  const marker = `flow marker ${Date.now()}`;
  const commitMessage = `add ${marker}`;

  await page.goto('/');
  await page.getByRole('searchbox', { name: 'Search scopes and memories' }).fill('widget-naming');
  const treeItem = page.locator('sl-tree-item[data-target="/memories/widget-naming"]');
  await expect(treeItem).toBeVisible();
  await treeItem.click();
  await expect(page).toHaveURL(/\/memories\/widget-naming$/);

  await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
  await typeAtEnd(page, marker);
  await save(page, commitMessage);

  // The document now carries the new text, and the memory is still open.
  await expect(page.locator('.milkdown .ProseMirror')).toContainText(marker);

  await page.getByRole('tab', { name: 'History' }).click();
  const commitLink = page.getByRole('link', { name: commitMessage });
  await expect(commitLink).toBeVisible();
  await commitLink.click();
  await expect(page.locator('pre.diff')).toContainText(`+${marker}`);
});

test('a memory whose body nobody touched is written back with the body reformatted', async ({
  page,
}) => {
  const before = await storedBody(page, 'reading-list');
  await openMemory(page, 'reading-list');

  // Only a field changes; the body must go back as the bytes that were loaded.
  await page.getByRole('button', { name: 'Edit Description' }).click();
  const description = page.locator('.page-header sl-input input');
  await description.fill(`untouched body check ${Date.now()}`);
  const sent = page.waitForRequest(
    (request) => request.method() === 'PUT' && request.url().includes('/api/memories/reading-list'),
  );
  await save(page, 'change only the description');

  const payload = JSON.parse((await sent).postData() ?? '{}') as { body: string };
  expect(payload.body).toBe(before);
  expect(await storedBody(page, 'reading-list')).toBe(before);
});

test('a wiki link is escaped when the document is edited somewhere else', async ({ page }) => {
  const marker = `wiki check ${Date.now()}`;
  await openMemory(page, 'reading-list');
  await expect(page.locator('.milkdown .ProseMirror')).toContainText('[[rocket-stages]]');

  await typeAtEnd(page, marker);
  await save(page, `add ${marker}`);

  const body = await storedBody(page, 'reading-list');
  expect(body).toContain('[[rocket-stages]]');
  expect(body).not.toContain('\\[\\[');
  expect(body).toContain(marker);
});

test('a write the server has already moved past is accepted, losing the other version', async ({
  page,
}) => {
  await openMemory(page, 'bench-power');
  await page.getByRole('button', { name: 'Edit Description' }).click();
  await page.locator('.page-header sl-input input').fill(`conflict check ${Date.now()}`);

  // Another writer changes the same memory while this page is open.
  const current = await page.request.get('/api/memories/bench-power');
  const doc = (await current.json()) as Record<string, unknown>;
  const sideWrite = await page.request.put('/api/memories/bench-power', {
    data: {
      description: doc.description,
      kind: doc.kind,
      scopes: doc.scopes,
      source: doc.source,
      body: `${String(doc.body)}\nthe other writer version of the line\n`,
      base_version: doc.version,
      author: 'other',
      message: 'change from another writer',
    },
  });
  expect(sideWrite.status()).toBe(200);

  const bar = page.locator('.commit-bar');
  await bar.locator('input').fill('the browser edit');
  await page.getByRole('button', { name: 'Save', exact: true }).click();

  const conflict = page.locator('.conflict');
  await expect(conflict).toBeVisible();
  await expect(conflict).toContainText('the other writer version of the line');
  await expect(conflict).toContainText('Loaded version');
});

test('a memory id with slashes loses its path when the address is loaded directly', async ({
  page,
}) => {
  await page.goto('/memories/sessions/alpha/session-1/notes');
  await expect(page.locator('.page-name')).toContainText('sessions/alpha/session-1/notes');
  await expect(page.locator('.milkdown .ProseMirror')).toContainText('bracket rework');
  await expect(page.locator('sl-tree-item[selected]')).toHaveCount(1);
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
  const firstCommit = page.locator('table.data tbody tr td:first-child a').first();
  await expect(firstCommit).toBeVisible();
  const href = await firstCommit.getAttribute('href');
  expect(href).toMatch(/\/memories\/widget-naming\/history\/[0-9a-f]{40}/);

  await page.goto(href ?? '/');
  await expect(page.locator('pre.diff')).toBeVisible();
  await expect(page.locator('pre.content')).toBeVisible();
});

test('a scope trigger is not editable on the scope page itself', async ({ page }) => {
  await page.goto('/scopes/rocketry');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('rocketry');
  await page.getByRole('button', { name: 'Edit Triggers' }).click();
  const pattern = page.locator('fmn-trigger-rows sl-input.pattern input').first();
  await expect(pattern).toBeVisible();
  // A pattern that is new on every run, so the page has something to save.
  const added = `thrust${Date.now()}`;
  await pattern.fill(`\\brocket(s|ry)?\\b|\\b${added}\\b`);

  const bar = page.locator('.commit-bar');
  await expect(bar).toBeVisible();
  await bar.locator('input').fill('widen the rocketry trigger');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(bar).toBeHidden();
  await expect(page.locator('table.data')).toContainText(added);
});

test('a scope created from the tree is missing from the tree and from the API', async ({ page }) => {
  const id = `probe-scope-${Date.now()}`;
  await page.goto('/');
  await page.getByRole('button', { name: 'New scope or memory' }).click();
  await page.getByRole('menuitem', { name: 'New scope' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('New scope');

  await page.getByRole('textbox', { name: 'Id', exact: true }).fill(id);
  await page.getByRole('button', { name: 'Add trigger' }).click();
  await page.locator('fmn-trigger-rows sl-input.pattern input').first().fill('\\bprobe\\b');
  await page.locator('.commit-bar input').fill(`add the ${id} scope`);
  await page.getByRole('button', { name: 'Create', exact: true }).click();

  await expect(page).toHaveURL(new RegExp(`/scopes/${id}$`));
  await expect(page.locator(`sl-tree-item[data-target="/scopes/${id}"]`)).toBeVisible();
  const scopes = (await (await page.request.get('/api/scopes')).json()) as { id: string }[];
  expect(scopes.map((scope) => scope.id)).toContain(id);
});

test('a memory created from a scope context menu is created outside that scope', async ({ page }) => {
  const id = `probe-memory-${Date.now()}`;
  await page.goto('/');
  await page
    .locator('sl-tree-item[data-target="/scopes/widgets"] > .tree-row')
    .click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'New memory in this scope' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('New memory');

  // The scope the menu came from is already a chip in the Scopes field.
  await expect(page.locator('fmn-tag-field sl-tag')).toHaveText(['widgets']);
  await page.getByRole('textbox', { name: 'Id', exact: true }).fill(id);
  await page
    .getByRole('textbox', { name: 'Description', exact: true })
    .fill('a memory made from the tree');
  await page.locator('.milkdown .ProseMirror').click();
  await page.keyboard.insertText('Made from the context menu.');
  await page.locator('.commit-bar input').fill(`add ${id}`);
  await page.getByRole('button', { name: 'Create', exact: true }).click();

  await expect(page).toHaveURL(new RegExp(`/memories/${id}$`));
  const doc = (await (await page.request.get(`/api/memories/${id}`)).json()) as { scopes: string[] };
  expect(doc.scopes).toEqual(['widgets']);
});

test('a memory deleted from the tree menu stays in the store', async ({ page }) => {
  const id = `probe-delete-${Date.now()}`;
  const created = await page.request.post('/api/memories', {
    data: {
      id,
      description: 'a memory made to be deleted',
      kind: 'knowledge',
      scopes: ['global'],
      source: 'user',
      body: `# ${id}\n\nMade to be deleted.\n`,
      author: 'wiki',
      message: `add ${id}`,
    },
  });
  expect(created.status()).toBe(200);

  // The delete route is new; until the server has it there is nothing to test here.
  const probe = await page.request.fetch(`/api/memories/${id}`, {
    method: 'DELETE',
    data: { base_version: 'f'.repeat(40), author: 'wiki', message: 'probe' },
  });
  test.skip(probe.status() === 405, 'the server has no delete route yet');

  await page.goto('/');
  await page.getByRole('searchbox', { name: 'Search scopes and memories' }).fill(id);
  await page
    .locator(`sl-tree-item[data-target="/memories/${id}"] > .tree-row`)
    .click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Delete memory' }).click();

  const panel = page.locator('sl-details[open] .delete-message input');
  await expect(panel).toBeVisible();
  await panel.fill(`delete ${id}`);
  await page.getByRole('button', { name: 'Delete memory' }).click();

  await expect(page).toHaveURL(/\/$/);
  const gone = await page.request.get(`/api/memories/${id}`);
  expect(gone.status()).toBe(404);
});

/** The scope delete route is new; until the server has it there is nothing to test. */
async function scopeDeleteReady(page: Page): Promise<boolean> {
  const probe = await page.request.fetch('/api/scopes/probe-absent-scope', {
    method: 'DELETE',
    data: { base_version: 'f'.repeat(40), author: 'wiki', message: 'probe' },
  });
  return probe.status() !== 405;
}

test('a scope picked from the suggestions is not added to the memory', async ({ page }) => {
  await openMemory(page, 'bench-power');
  await page.getByRole('button', { name: 'Edit Scopes' }).click();

  const field = page.locator('fmn-tag-field sl-input input');
  await expect(field).toBeVisible();
  await expect(page.locator('fmn-tag-field sl-tag')).toHaveText(['global']);

  // Typed, chosen from the list with the keyboard.
  await field.fill('rocket');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(page.locator('fmn-tag-field sl-tag')).toHaveText(['global', 'rocketry']);

  await save(page, 'put bench-power in rocketry as well');
  const doc = (await (await page.request.get('/api/memories/bench-power')).json()) as {
    scopes: string[];
  };
  expect(doc.scopes).toEqual(['global', 'rocketry']);

  // Backspace takes the last chip back.
  await page.getByRole('button', { name: 'Edit Scopes' }).click();
  await page.locator('fmn-tag-field sl-input input').click();
  await page.keyboard.press('Backspace');
  await expect(page.locator('fmn-tag-field sl-tag')).toHaveText(['global']);
});

test('a scope nothing refers to is kept when it is deleted from the tree', async ({ page }) => {
  const id = `probe-lonely-${Date.now()}`;
  const created = await page.request.post('/api/scopes', {
    data: { id, implies: [], triggers: [], author: 'wiki', message: `add ${id}` },
  });
  expect(created.status()).toBe(200);
  test.skip(!(await scopeDeleteReady(page)), 'the server has no scope delete route yet');

  await page.goto('/');
  await page.getByRole('searchbox', { name: 'Search scopes and memories' }).fill(id);
  await page.locator(`sl-tree-item[data-target="/scopes/${id}"] > .tree-row`).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Delete scope' }).click();

  const field = page.locator('sl-details[open] .delete-message input');
  await expect(field).toBeVisible();
  await field.fill(`delete ${id}`);
  await page.getByRole('button', { name: 'Delete scope' }).click();

  await expect(page).toHaveURL(/\/$/);
  await expect(page.locator(`sl-tree-item[data-target="/scopes/${id}"]`)).toHaveCount(0);
  const scopes = (await (await page.request.get('/api/scopes')).json()) as { id: string }[];
  expect(scopes.map((scope) => scope.id)).not.toContain(id);
});

test('a scope a memory still lists is deleted without saying what refers to it', async ({ page }) => {
  test.skip(!(await scopeDeleteReady(page)), 'the server has no scope delete route yet');

  await page.goto('/scopes/widgets');
  await page.locator('sl-details').filter({ hasText: 'Delete' }).first().click();
  const field = page.locator('sl-details[open] .delete-message input');
  await expect(field).toBeVisible();
  await field.fill('remove the widgets scope');
  await page.getByRole('button', { name: 'Delete scope' }).click();

  // The scope stays, and the page names the file that still refers to it.
  const errors = page.locator('fmn-validation-errors sl-alert');
  await expect(errors).toBeVisible();
  await expect(errors).toContainText('widget-naming');
  await expect(page).toHaveURL(/\/scopes\/widgets$/);
  const scopes = (await (await page.request.get('/api/scopes')).json()) as { id: string }[];
  expect(scopes.map((scope) => scope.id)).toContain('widgets');
});

test('the statistics page reports one figure per memory instead of one per delivery form', async ({
  page,
}) => {
  await page.goto('/stats');
  await expect(
    page.getByRole('columnheader', { name: 'Shown as index line', exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole('columnheader', { name: 'Shown in full, new', exact: true }),
  ).toBeVisible();
  await expect(page.locator('table.stats').first().locator('tbody tr').first()).toBeVisible();
});

test('a trigger created in the UI is written with an on line it was never given', async ({ page }) => {
  const id = `any-scope-${Date.now()}`;
  await page.goto('/scopes/new');
  await page.getByRole('textbox', { name: 'Id', exact: true }).fill(id);
  await page.getByRole('button', { name: 'Add trigger' }).click();

  const row = page.locator('fmn-trigger-rows .trigger-row').first();
  await expect(row.locator('sl-select')).toHaveJSProperty('value', 'any');
  await row.locator('sl-input.pattern input').fill('\\bprobe\\b');
  await page.locator('.commit-bar input').fill(`add the ${id} scope`);
  await page.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/scopes/${id}$`));

  const scope = (await (await page.request.get(`/api/scopes/${id}`)).json()) as {
    triggers: Record<string, unknown>[];
  };
  expect(scope.triggers).toEqual([{ pattern: '\\bprobe\\b' }]);
  await expect(page.locator('table.data')).toContainText('any');
});

test('a pattern that is not a regex is accepted without a word about it', async ({ page }) => {
  await page.goto('/scopes/widgets');
  await page.getByRole('button', { name: 'Edit Triggers' }).click();
  const row = page.locator('fmn-trigger-rows .trigger-row').first();
  await row.locator('sl-input.pattern input').fill('\\bwidget(s?\\b');

  // The message is the regex crate's own, so only its first line is asserted on.
  await expect(row.locator('.field-error')).toContainText('regex parse error');

  await row.locator('sl-input.pattern input').fill('\\bwidgets?\\b');
  await expect(row.locator('.field-error')).toHaveCount(0);
});

test('the machine field offers nothing and keeps nothing', async ({ page }) => {
  const id = `machine-scope-${Date.now()}`;
  await page.goto('/scopes/new');
  await page.getByRole('textbox', { name: 'Id', exact: true }).fill(id);
  await page.getByRole('button', { name: 'Add trigger' }).click();

  const row = page.locator('fmn-trigger-rows .trigger-row').first();
  await row.locator('sl-input.pattern input').fill('\\bprobe\\b');
  const machine = row.locator('fmn-tag-field sl-input input');
  await machine.click();
  const offered = row.locator('fmn-tag-field sl-menu-item');
  await expect(offered.first()).toBeVisible();
  await expect(offered).toContainText(['alpha', 'beta']);
  // The list is picked from with the keyboard, which is the path a mouse click ends
  // in as well: the highlighted row is the one Enter takes.
  await machine.fill('bet');
  await expect(offered).toHaveCount(1);
  await machine.press('Enter');
  await expect(row.locator('fmn-tag-field sl-tag')).toHaveText('beta');

  await page.locator('.commit-bar input').fill(`add the ${id} scope`);
  await page.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/scopes/${id}$`));

  const scope = (await (await page.request.get(`/api/scopes/${id}`)).json()) as {
    triggers: Record<string, unknown>[];
  };
  expect(scope.triggers).toEqual([{ pattern: '\\bprobe\\b', machine: 'beta' }]);
});
