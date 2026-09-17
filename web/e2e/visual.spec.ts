import { expect, test, type Page } from '@playwright/test';
import {
  iconColours,
  measureContrast,
  metricsOf,
  pageOverflow,
  paneFacts,
} from './appearance';

// Detects the failures that only the rendered result shows: text that does not meet
// WCAG AA against what is behind it, type or controls drifting off the intended
// scale, a pane that overlaps or clips another, icons drawn in more than one colour,
// and any change to the look that nobody reviewed. The screenshots are a recorded
// human judgment: they were looked at once and accepted, so a later change that
// alters the look fails here until it is looked at again.

interface Size {
  name: string;
  width: number;
  height: number;
}

const sizes: Size[] = [
  { name: 'phone', width: 390, height: 844 },
  { name: 'tablet', width: 820, height: 1180 },
  { name: 'laptop', width: 1440, height: 900 },
  { name: 'wide', width: 2560, height: 1440 },
];

/** Text whose pair has to meet AA on every page that shows it. */
const textSelectors = [
  '.app-bar .brand',
  '.app-menu a',
  '.sidebar-title span',
  '.tree-row .tree-label',
  '.tree-row.category .tree-label',
  '.tree-row .tree-count',
  '.page-name span',
  '.page-header .description',
  '.field-label',
  '.field-value .value-text',
  '.infobox code',
  '.milkdown .ProseMirror p',
  '.milkdown .ProseMirror h1',
];

const tableSelectors = ['table.data th', 'table.data td', 'table.data td a', 'h2'];

async function settle(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(350);
}

async function checkContrast(page: Page, selectors: string[]): Promise<void> {
  const measured = await page.evaluate(measureContrast, selectors);
  expect(measured.length).toBeGreaterThan(0);
  for (const entry of measured) {
    expect(
      entry.ratio,
      `${entry.selector} ${entry.color} on ${entry.background} at ${entry.fontSize}px/${entry.fontWeight}`,
    ).toBeGreaterThanOrEqual(entry.required);
  }
}

/**
 * What the clock decides: the moment a memory was last shown, the day a call was
 * stopped, and the microseconds an event took. Those cells are painted over, so that
 * a run an hour or a day later still compares. Everything else on the page is what
 * the seeded store and the fixed hook sequence say, and is compared as it is drawn.
 */
function clockValues(page: Page) {
  return [page.locator('.moment'), page.locator('.table-wrap:has(table.latency)')];
}

async function checkNoOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(pageOverflow);
  expect(overflow.scrollWidth, 'the page scrolls sideways').toBeLessThanOrEqual(
    overflow.innerWidth + 1,
  );
}

for (const size of sizes) {
  for (const scheme of ['light', 'dark'] as const) {
    test.describe(`${size.name} ${scheme}`, () => {
      test.use({ viewport: { width: size.width, height: size.height }, colorScheme: scheme });

      test('a page reports text that fails AA against its background', async ({ page }) => {
        await page.goto('/memories/widget-naming');
        await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
        await settle(page);
        await checkContrast(page, textSelectors);
        await checkNoOverflow(page);

        await page.goto('/');
        await settle(page);
        await checkContrast(page, tableSelectors);
        await checkNoOverflow(page);

        await page.goto('/stats');
        await settle(page);
        await checkContrast(page, ['table.stats th', 'table.stats td']);
        await checkNoOverflow(page);

        await page.goto('/scopes/widgets');
        await settle(page);
        await checkContrast(page, ['.memory-index a', '.muted', 'table.data td code']);
        await checkNoOverflow(page);
      });

      test('the type and the controls drift off the intended scale', async ({ page }) => {
        await page.goto('/memories/widget-naming');
        await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
        await settle(page);

        const document_ = await page.evaluate(metricsOf, '.milkdown .ProseMirror p');
        expect(document_?.fontSize, 'the document body').toBeGreaterThanOrEqual(15);
        expect(document_?.fontSize, 'the document body').toBeLessThanOrEqual(17);

        const heading = await page.evaluate(metricsOf, '.milkdown .ProseMirror h1');
        expect(heading?.fontSize, 'the document title').toBeGreaterThanOrEqual(24);
        expect(heading?.fontSize, 'the document title').toBeLessThanOrEqual(34);

        const label = await page.evaluate(metricsOf, '.tree-row .tree-label');
        expect(label?.fontSize, 'the tree rows').toBeGreaterThanOrEqual(13);
        expect(label?.fontSize, 'the tree rows').toBeLessThanOrEqual(15.5);

        // On a narrow screen the hierarchy lives in the drawer, so it has to be
        // opened before its controls can be measured.
        if (size.width <= 900) {
          await page.getByRole('button', { name: 'Navigation' }).click();
          await page.waitForTimeout(400);
        }
        const search = await page.evaluate(metricsOf, '.sidebar-header sl-input');
        expect(search?.height, 'the search box').toBeGreaterThanOrEqual(26);
        expect(search?.height, 'the search box').toBeLessThanOrEqual(36);
      });

      test('the two panes are not told apart', async ({ page }) => {
        await page.goto('/memories/widget-naming');
        await settle(page);
        const facts = await page.evaluate(paneFacts);
        if (size.width > 900) {
          expect(facts.sidebar, 'the sidebar').not.toBeNull();
          expect(facts.content, 'the content').not.toBeNull();
          const sidebar = facts.sidebar;
          const content = facts.content;
          if (sidebar !== null && content !== null) {
            expect(sidebar.x + sidebar.width, 'the panes overlap').toBeLessThanOrEqual(
              content.x + 1,
            );
          }
          // Either a different surface or a visible border separates them.
          const separated =
            facts.borderWidth >= 1 || facts.sidebarBackground !== facts.contentBackground;
          expect(separated, 'nothing separates the panes').toBe(true);
          expect(facts.borderColour).not.toBe(facts.sidebarBackground);
        }
      });

      test('the icons are drawn in more than one colour', async ({ page }) => {
        await page.goto('/memories/widget-naming');
        await settle(page);
        if (size.width <= 900) {
          await page.getByRole('button', { name: 'Navigation' }).click();
          await page.waitForTimeout(400);
        }
        const colours = await page.evaluate(iconColours, [
          '.tree-row sl-icon',
          'fmn-kind-icon sl-icon',
          '.page-name sl-icon',
        ]);
        expect(colours, 'the icons use more than one colour').toHaveLength(1);
      });
    });
  }
}

// The reviewed look, one image per state. A change in any of them fails until the
// new image is looked at and accepted.

const laptop = { width: 1440, height: 900 };

for (const scheme of ['light', 'dark'] as const) {
  test.describe(`look ${scheme}`, () => {
    // The images were recorded in the container, against the store the global setup
    // seeds: an outside server named by FORGETMENOT_URL is not that store, and a run
    // outside the container is not that machine.
    test.skip(
      process.env.FORGETMENOT_URL !== undefined || process.env.FMN_SKIP_SCREENSHOTS !== undefined,
      'the images belong to the container and its fixture store',
    );
    test.use({ viewport: laptop, colorScheme: scheme });

    test('the overview looks as reviewed', async ({ page }) => {
      await page.goto('/');
      await expect(page.locator('table.data').first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`overview-laptop-${scheme}.png`);
    });

    test('a memory page looks as reviewed', async ({ page }) => {
      await page.goto('/memories/rocket-stages');
      await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`memory-laptop-${scheme}.png`);
    });

    test('an edited memory looks as reviewed', async ({ page }) => {
      await page.goto('/memories/rocket-stages');
      await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
      await page.getByRole('button', { name: 'Edit Description' }).click();
      await settle(page);
      await expect(page.locator('.commit-bar')).toBeHidden();
      await page.locator('.page-header sl-input input').fill('a description under review');
      await expect(page.locator('.commit-bar')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`memory-editing-laptop-${scheme}.png`);
    });

    test('the history of a memory looks as reviewed', async ({ page }) => {
      await page.goto('/memories/rocket-stages/history');
      await expect(page.locator('table.data')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`history-laptop-${scheme}.png`);
    });

    test('the history of the store looks as reviewed', async ({ page }) => {
      await page.goto('/history');
      await expect(page.locator('table.data')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`store-history-laptop-${scheme}.png`);
    });

    test('the page of one commit looks as reviewed', async ({ page, request }) => {
      // The oldest commit is the one the fixture seeds, whose id is fixed by the
      // dates in `seedCommit`; it is read here rather than written down, so a
      // change to what is seeded does not leave a dead address behind.
      const history = (await (await request.get('/api/history')).json()) as {
        commits: { oid: string }[];
      };
      const seeded = history.commits.at(-1)?.oid ?? '';
      await page.goto(`/history/${seeded}`);
      await expect(page.locator('fmn-diff').first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`store-commit-laptop-${scheme}.png`);
    });

    test('a scope page looks as reviewed', async ({ page }) => {
      await page.goto('/scopes/widgets');
      await expect(page.locator('fmn-trigger-table')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`scope-laptop-${scheme}.png`);
    });

    test('a trigger row under the pointer looks as reviewed', async ({ page }) => {
      // The table is the same table read or written; this is what it shows when the
      // pointer is on a row, which is when its controls are drawn.
      await page.goto('/scopes/widgets');
      await expect(page.locator('fmn-trigger-table')).toBeVisible();
      await page.locator('fmn-trigger-table .editable-row').nth(1).hover();
      await settle(page);
      await expect(page).toHaveScreenshot(`scope-editing-laptop-${scheme}.png`);
    });

    test('the menu the tree opens looks as reviewed', async ({ page }) => {
      // On a memory page, so nothing the server counts is behind the menu.
      await page.goto('/memories/rocket-stages');
      await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
      await page
        .locator('sl-tree-item[data-target="/scopes/widgets"] > .tree-row')
        .click({ button: 'right' });
      await expect(page.getByRole('menuitem', { name: 'New memory in this scope' })).toBeVisible();
      await settle(page);
      // The menu itself, rather than the page: a page capture of an overlay that is
      // still animating catches it half drawn.
      await page.waitForTimeout(400);
      await expect(page.locator('.context-menu sl-menu')).toHaveScreenshot(
        `tree-menu-laptop-${scheme}.png`,
        { animations: 'allow' },
      );
    });

    test('the statistics look as reviewed', async ({ page }) => {
      await page.goto('/stats');
      await expect(page.locator('table.stats').first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`stats-laptop-${scheme}.png`, {
        mask: clockValues(page),
      });
    });

    test('the contexts look as reviewed', async ({ page }) => {
      await page.goto('/contexts');
      await expect(page.locator('table.data')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`contexts-laptop-${scheme}.png`, {
        mask: clockValues(page),
      });
    });

    test('the prompt of a context looks as reviewed', async ({ page }) => {
      // Everything the session's scopes hold, which is the longer of the two texts
      // and the one a reader opens the page for.
      await page.goto('/contexts/alpha/session-1/prompt?mode=all');
      await expect(page.locator('pre.prompt')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`context-prompt-laptop-${scheme}.png`);
    });

    test('the settings page looks as reviewed', async ({ page }) => {
      await page.goto('/settings');
      await expect(page.locator('.settings-list')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`settings-laptop-${scheme}.png`);
    });

    test('the new memory page looks as reviewed', async ({ page }) => {
      await page.goto('/memories/new');
      await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`new-laptop-${scheme}.png`);
    });
  });
}

for (const size of sizes) {
  for (const scheme of ['light', 'dark'] as const) {
    test.describe(`look ${size.name} ${scheme}`, () => {
      // The images were recorded in the container, against the store the global
      // setup seeds: an outside server named by FORGETMENOT_URL is not that store,
      // and a run outside the container is not that machine.
      test.skip(
        process.env.FORGETMENOT_URL !== undefined || process.env.FMN_SKIP_SCREENSHOTS !== undefined,
        'the images belong to the container and its fixture store',
      );
      test.use({ viewport: { width: size.width, height: size.height }, colorScheme: scheme });

      test('the layout at this size looks as reviewed', async ({ page }) => {
        await page.goto('/memories/rocket-stages');
        await expect(page.locator('.milkdown .ProseMirror')).toBeVisible();
        await settle(page);
        await expect(page).toHaveScreenshot(`layout-${size.name}-${scheme}.png`);
      });

      test('the statistics at this size look as reviewed', async ({ page }) => {
        await page.goto('/stats');
        await expect(page.locator('table.stats').first()).toBeVisible();
        await settle(page);
        await expect(page).toHaveScreenshot(`stats-${size.name}-${scheme}.png`, {
          mask: clockValues(page),
        });
      });
    });
  }
}

test.describe('look phone drawer', () => {
  // The images were recorded in the container, against the store the global setup
  // seeds: an outside server named by FORGETMENOT_URL is not that store, and a run
  // outside the container is not that machine.
  test.skip(
    process.env.FORGETMENOT_URL !== undefined || process.env.FMN_SKIP_SCREENSHOTS !== undefined,
    'the images belong to the container and its fixture store',
  );
  test.use({ viewport: { width: 390, height: 844 } });

  test('the hierarchy drawer looks as reviewed', async ({ page }) => {
    await page.goto('/memories/rocket-stages');
    await settle(page);
    await page.getByRole('button', { name: 'Navigation' }).click();
    await page.waitForTimeout(500);
    await expect(page).toHaveScreenshot('drawer-phone-light.png');
  });
});
