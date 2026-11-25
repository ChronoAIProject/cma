import { paint } from '../../../.github/scripts/scale-tests/timing/lib/flows.mjs';
import {
  clock,
  delay,
  requireCheck,
} from '../../../.github/scripts/scale-tests/timing/lib/core.mjs';

const base = '/api/v1/admin/usage/metrics';
const reportIdPattern = /^umr_[a-f0-9]{32}$/;

export function isReportRequest(path, method) {
  if (method !== 'POST') return false;
  return (
    [
      base + '/reports',
      base + '/options',
      base + '/initial/view',
      base + '/initial/reports',
    ].includes(path) ||
    (path.startsWith(base + '/initial/reports/') &&
      reportIdPattern.test(path.slice((base + '/initial/reports/').length)))
  );
}

export function reportObservation(path, method, status, body) {
  if (status !== 200) return null;
  if (method === 'POST' && isReportRequest(path, method) && path.startsWith(base + '/initial/')) {
    const id = (body?.view?.report ?? body?.report)?.report_id;
    const page = body?.initial?.rows;
    if (reportIdPattern.test(id) && page?.report_id === id && Array.isArray(page.items)) {
      return { id, count: page.items.length, projected: true };
    }
  }
  if (method === 'GET' && path.startsWith(base + '/reports/') && path.endsWith('/sandboxes')) {
    const id = path.slice((base + '/reports/').length, -'/sandboxes'.length);
    if (reportIdPattern.test(id) && body?.report_id === id && Array.isArray(body.items)) {
      return { id, count: body.items.length, projected: false };
    }
  }
  return null;
}

export async function measureAdminReport(page, origin) {
  let observed;
  let responseSequence = 0;
  let observedSequence = 0;
  await page.route('**/*', (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (
      url.origin !== origin ||
      (!['GET', 'HEAD', 'OPTIONS'].includes(request.method()) &&
        !isReportRequest(url.pathname, request.method()))
    ) {
      return route.abort('blockedbyclient');
    }
    return route.continue();
  });
  await page.routeWebSocket('**/*', (socket) => socket.close());

  const observe = async (response) => {
    const { origin: from, pathname } = new URL(response.url());
    if (from !== origin || !pathname.startsWith(base + '/') || response.status() !== 200) return;
    const sequence = ++responseSequence;
    const projection = reportObservation(
      pathname,
      response.request().method(),
      response.status(),
      await response.json().catch(() => null),
    );
    if (projection && sequence > observedSequence) {
      observed = projection;
      observedSequence = sequence;
    }
  };
  page.on('response', observe);
  try {
    const response = await page.goto(origin + '/operations?tab=sandboxes&range=7d', {
      waitUntil: 'domcontentloaded',
      timeout: 30000,
    });
    requireCheck(response?.status() === 200, 'operations_navigation_failed', response?.status());
    const dashboard = page.locator(
      '[aria-label="Operations resource dashboard"][data-usage-requested-range="7d"][data-usage-state="committed"]',
    );
    const inventory = dashboard.getByRole('region', { name: 'Usage inventory', exact: true });
    const capacity = dashboard.getByRole('region', { name: 'Allocated capacity now', exact: true });
    const deadline = clock() + 60000;
    while (clock() < deadline) {
      const candidate = observed;
      if (
        candidate &&
        (await capacity.isVisible()) &&
        (await inventory.isVisible()) &&
        !(await dashboard
          .getByText(
            /Preparing usage report|Loading usage report|Loading usage page|Showing the previous selection/,
          )
          .first()
          .isVisible())
      ) {
        const matches =
          candidate.count === 0
            ? await inventory
                .getByText('No Sandboxes match this selection.', { exact: true })
                .isVisible()
            : (await inventory.locator('tbody tr:visible').count()) === candidate.count;
        if (matches && candidate === observed) {
          await paint(page);
          return {
            summary_rendered: true,
            first_page_rendered: true,
            row_count: candidate.count,
            projected: candidate.projected,
          };
        }
      }
      await delay(50);
    }
    requireCheck(false, 'usage_inventory_timeout');
  } finally {
    page.off('response', observe);
  }
}
