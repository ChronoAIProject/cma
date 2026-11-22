import { expect, type Page } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';

export const weeklySummary = `# Week 47 project summary

The sample project made progress on three priorities this week. The team reviewed the findings and agreed on the next round of work.

## Completed this week

- Reviewed the onboarding screens
- Prepared the research brief
- Collected feedback on the weekly summary

## Open questions

Which changes should go into the next iteration? Bring the remaining feedback to the next planning session.
`;

export class RedesignDocument extends LocalDocuments {
  applications = 0;
  loseNextResponse = true;
  refusal?: number;
  private receipts = new Map<string, { body: string; document: LocalDocuments['document'] }>();

  override async route(page: Page, baseURL: string) {
    this.document.title = 'Weekly project draft';
    this.markdown = weeklySummary;
    await super.route(page, baseURL);
    await page.route(`**/api/v1/documents/${documentId}`, async (route) => {
      const request = route.request();
      if (request.method() !== 'PATCH') return route.fallback();
      const body = request.postDataJSON() as Record<string, unknown>;
      const key = request.headers()['idempotency-key'];
      this.writes.push({ method: 'PATCH', path: new URL(request.url()).pathname, body, key });
      await this.saveGate;
      if (this.refusal)
        return route.fulfill({
          status: this.refusal,
          json: { detail: `Save refused (${this.refusal})` },
        });
      const previous = key && this.receipts.get(key);
      if (previous) {
        expect(JSON.stringify(body)).toBe(previous.body);
        return route.fulfill({ json: previous.document });
      }
      expect(key).toBeTruthy();
      expect(body.expected_revision).toBe(this.document.revision);
      this.applications++;
      this.document = {
        ...this.document,
        title: String(body.title),
        revision: this.document.revision + 1,
        source_revision: this.document.revision + 1,
      };
      this.markdown = String(body.markdown);
      this.receipts.set(key!, { body: JSON.stringify(body), document: { ...this.document } });
      if (this.loseNextResponse) {
        this.loseNextResponse = false;
        return route.abort('connectionreset');
      }
      return route.fulfill({ json: this.document });
    });
  }
}

export const titleOf = (page: Page) => page.getByRole('textbox', { name: 'Document title' });
export const contentOf = (page: Page) =>
  page.getByRole('textbox', { name: 'Document content', exact: true });
export const recoveryOf = (page: Page) =>
  page.getByRole('region', { name: 'We can’t confirm your last save' });

export async function openRedesignDocument(
  page: Page,
  baseURL: string,
  fixture = new RedesignDocument(),
) {
  await page.clock.install({ time: new Date('2026-11-22T08:00:00Z') });
  await fixture.route(page, baseURL);
  await page.goto(`/artifacts/${documentId}`);
  await expect(contentOf(page)).toBeVisible();
  return fixture;
}

export async function loseSaveResponse(page: Page, fixture: RedesignDocument) {
  await titleOf(page).fill('Weekly project summary');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(recoveryOf(page)).toBeVisible();
  await expect(page.locator('.document-workspace__save-status')).toContainText(
    'Save not confirmed',
  );
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.applications).toBe(1);
}

export async function replaceContent(page: Page, text: string) {
  await contentOf(page).fill(text);
}

export async function installClipboard(page: Page, fail = false) {
  await page.addInitScript((reject) => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          if (reject) throw new Error('Clipboard denied');
          (window as unknown as { copiedDocumentText: string }).copiedDocumentText = text;
        },
      },
    });
  }, fail);
}
