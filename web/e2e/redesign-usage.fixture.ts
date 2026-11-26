import type { Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { metricRow } from '../src/usage/metrics/fixtures.test-support';

export async function setupUsageRedesign(
  page: Page,
  baseURL: string,
  { theme = 'dark', measured = false }: { theme?: string; measured?: boolean } = {},
) {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  metrics.measuredVisible = measured;
  metrics.presentationRows = [
    ['Example environment', 18.5, true, 0],
    ['Research environment', 15, false, 7200],
    ['Planning environment', 8, true, 0],
  ].map(([name, cpu, complete, projected], index) => ({
    ...metricRow,
    sandbox_id: `sbx-redesign-${index}`,
    name: String(name),
    allocated_period: {
      ...metricRow.allocated_period,
      cpu_core_hours: Number(cpu),
      complete: Boolean(complete),
      unknown_seconds: 0,
      projected_tail_seconds: Number(projected),
    },
  }));
  await boundary.route(page, baseURL);
  await metrics.route(page);
  await metrics.session(page);
  await page.addInitScript(
    (appearance) => localStorage.setItem('cma:appearance', appearance),
    theme,
  );
  return { boundary, metrics };
}
