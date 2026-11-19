import { test, expect, type Page } from '@playwright/test';
import { LocalDocuments } from './documents.fixture';

const scopes = [
  { value: 'all', label: 'All', heading: 'Your library' },
  { value: 'owned', label: 'Yours', heading: 'Your library' },
  { value: 'shared', label: 'Shared with you', heading: 'Shared documents' },
] as const;
const reads = (fixture: LocalDocuments) =>
  fixture.reads.filter((read) => read.path === '/api/v1/documents');
const lastQuery = (fixture: LocalDocuments) => new URLSearchParams(reads(fixture).at(-1)?.query);
const settled = async (page: Page) => {
  await expect(page.locator('.document-gallery')).toHaveAttribute('aria-busy', 'false');
};
const safe = (fixture: LocalDocuments) => {
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
};

for (const populated of [false, true]) {
  for (const scope of scopes) {
    test(`LOCAL ${scope.value} treats Unicode whitespace as no query (populated=${String(populated)})`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalDocuments();
      fixture.registryDocuments = populated
        ? [{ ...fixture.document, access: scope.value === 'shared' ? 'read' : 'owner' }]
        : [];
      await fixture.route(page, baseURL!);
      await page.goto('/artifacts');
      await settled(page);
      await page.getByRole('button', { name: scope.label, exact: true }).click();
      await settled(page);
      const search = page.getByRole('searchbox', { name: 'Search artifacts' });
      for (const value of ['', ' ', '   ', '\u3000', '\u00a0', '\t', '\r\n', ' \u3000\u00a0\t ']) {
        await search.fill(value);
        await expect(page.getByRole('heading', { level: 2 })).toHaveText(scope.heading);
        await expect(page.getByText('No matching documents')).toHaveCount(0);
        if (populated)
          await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
        else await expect(page.getByText('A place for your next idea')).toBeVisible();
      }
      await settled(page);
      expect(lastQuery(fixture).get('scope')).toBe(scope.value);
      expect(lastQuery(fixture).has('q')).toBe(false);
      await page.screenshot({
        path: info.outputPath(`${scope.value}-${String(populated)}.png`),
        animations: 'disabled',
      });
      safe(fixture);
    });
  }
}

for (const query of ['', 'report']) {
  test(`LOCAL equivalent whitespace retains loaded pages for ${query || 'normal listing'}`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalDocuments();
    fixture.registryDocuments = ['a', 'b', 'c'].map((letter, index) => ({
      ...fixture.document,
      id: `doc_${letter.repeat(32)}`,
      title: `Report ${String(index + 1)}`,
    }));
    fixture.registryPageSize = 1;
    await page.clock.install();
    await fixture.route(page, baseURL!);
    await page.goto('/artifacts');
    await settled(page);
    const search = page.getByRole('searchbox', { name: 'Search artifacts' });
    if (query) {
      await search.fill(query);
      await expect.poll(() => lastQuery(fixture).get('q')).toBe(query);
      await settled(page);
    }
    await page.getByRole('button', { name: 'Load more' }).click();
    await expect(page.getByRole('link', { name: /Report 2/ })).toBeVisible();
    const count = reads(fixture).length;
    await search.fill(`  \u3000${query}\u00a0 `);
    await page.getByRole('button', { name: 'All', exact: true }).click();
    await page.clock.fastForward(250);
    await expect(page.getByRole('link', { name: /Report 1/ })).toBeVisible();
    await expect(page.getByRole('link', { name: /Report 2/ })).toBeVisible();
    await page.getByRole('button', { name: 'Load more' }).click();
    await expect(page.getByRole('link', { name: /Report 3/ })).toBeVisible();
    expect(reads(fixture)).toHaveLength(count + 1);
    expect(lastQuery(fixture).get('cursor')).toBe('2');
    expect(lastQuery(fixture).get('q')).toBe(query || null);

    await search.fill('  report  draft  ');
    await expect(page.getByText('No matching documents')).toBeVisible();
    expect(lastQuery(fixture).get('q')).toBe('report  draft');
    expect(lastQuery(fixture).has('cursor')).toBe(false);
    await search.fill('   ');
    await expect(page.getByRole('heading', { level: 2 })).toHaveText('Your library');
    await expect(page.getByRole('link', { name: /Report 1/ })).toBeVisible();
    expect(lastQuery(fixture).has('q')).toBe(false);
    expect(lastQuery(fixture).has('cursor')).toBe(false);
    safe(fixture);
  });
}

test('LOCAL blank-query errors remain errors and Retry recovers the current scope', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await settled(page);
  await page.getByRole('searchbox').fill('missing');
  await expect(page.getByText('No matching documents')).toBeVisible();
  fixture.registryFailures = 1;
  await page.getByRole('searchbox').fill('   ');
  await expect(page.getByRole('alert')).toContainText('Artifacts are unavailable');
  await expect(page.getByText('A place for your next idea')).toHaveCount(0);
  await expect(page.getByText('No matching documents')).toHaveCount(0);
  await page.getByRole('searchbox').fill('\u3000\u00a0');
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(lastQuery(fixture).has('q')).toBe(false);
  safe(fixture);
});

test('LOCAL an obsolete search cannot replace a newly selected scope', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  let release!: () => void;
  fixture.registryGate = {
    query: 'old',
    wait: new Promise<void>((resolve) => {
      release = resolve;
    }),
  };
  fixture.registryDocuments = [
    { ...fixture.document, title: 'Old owned result' },
    {
      ...fixture.document,
      id: `doc_${'b'.repeat(32)}`,
      title: 'Old shared result',
      access: 'read',
    },
  ];
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await settled(page);
  await page.getByRole('searchbox').fill('old');
  await expect.poll(() => lastQuery(fixture).get('q')).toBe('old');
  fixture.registryGate = undefined;
  await page.getByRole('button', { name: 'Shared with you' }).click();
  await expect(page.getByRole('link', { name: /Old shared result/ })).toBeVisible();
  release();
  await settled(page);
  await expect(page.getByRole('link', { name: /Old owned result/ })).toHaveCount(0);
  expect(lastQuery(fixture).get('scope')).toBe('shared');
  safe(fixture);
});

test('LOCAL actual multiline clipboard paste follows the browser search-input sanitization', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await settled(page);
  await page.evaluate(() => {
    const source = document.createElement('textarea');
    source.id = 'local-clipboard-source';
    source.value = ' \t\n\u3000\r\n\u00a0 ';
    document.body.append(source);
    source.select();
  });
  await page.keyboard.press('ControlOrMeta+C');
  const search = page.getByRole('searchbox', { name: 'Search artifacts' });
  await search.focus();
  await page.keyboard.press('ControlOrMeta+V');
  await expect.poll(() => search.inputValue()).not.toBe('');
  expect((await search.inputValue()).trim()).toBe('');
  await expect(page.getByRole('heading', { level: 2 })).toHaveText('Your library');
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  expect(lastQuery(fixture).has('q')).toBe(false);
  await page.locator('#local-clipboard-source').evaluate((node) => node.remove());
  safe(fixture);
});
