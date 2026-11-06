import type { Page } from '@playwright/test';
import type { UsageMetricQuery } from '@chronoai/cma-sdk';

export const filterOwners = [
  { value: '91892afa-5fa5-4f4f-9100-2b8d986d2f71', name: 'Alex Chen', hint: 'Joined 2026-11-01' },
  { value: '4ef54d11-f781-44ad-a301-1b5f574ca012', name: 'Alex Chen', hint: 'Joined 2026-11-02' },
  { value: 'a9013ad5-2702-4a35-8fef-f001e5b10a31', name: 'Unnamed owner', hint: '' },
];
export class LocalUsageFilterOptions {
  readonly reads: URL[] = [];
  failure = 0;
  delay = 0;
  count = 20001;
  readonly rows = Array.from({ length: this.count }, (_, index) => ({
    value: `sbx_${index.toString(16).padStart(32, '0')}`,
    name: `Research ${index.toString().padStart(5, '0')}`,
    owner: filterOwners[index % 3]!.value,
    runtime: index % 2 ? 'runtime-previous' : 'runtime-stable',
    status: index % 2 ? 'paused' : 'active',
  }));
  matching(input: UsageMetricQuery) {
    const filters = input.filters;
    return this.rows.filter(
      (row) =>
        (!filters?.owner || row.owner === filters.owner) &&
        (!filters?.sandbox_id || row.value === filters.sandbox_id) &&
        (!filters?.runtime_version || row.runtime === filters.runtime_version) &&
        (!filters?.statuses?.length ||
          filters.statuses.includes(row.status as 'active' | 'paused')),
    );
  }
  async route(page: Page) {
    await page.route('**/api/v1/admin/usage/metrics/options**', async (route) => {
      const url = new URL(route.request().url());
      const body = route.request().postDataJSON() as Record<string, string | number>;
      for (const [key, value] of Object.entries(body)) url.searchParams.set(key, String(value));
      this.reads.push(url);
      if (this.delay) await new Promise((resolve) => setTimeout(resolve, this.delay));
      if (this.failure)
        return route.fulfill({
          status: this.failure,
          json: { status: this.failure, title: 'Options unavailable', code: 'usage_unavailable' },
        });
      const q = url.searchParams;
      const kind = q.get('kind');
      const choices =
        kind === 'owner'
          ? filterOwners.map((owner) => ({
              ...owner,
              label: owner.name,
            }))
          : kind === 'runtime'
            ? ['runtime-stable', 'runtime-previous'].map((value) => ({
                value,
                name: value,
                label: value,
                hint: '',
              }))
            : this.rows
                .filter((row) => !q.get('owner') || row.owner === q.get('owner'))
                .map((row) => ({
                  ...row,
                  label: row.name,
                  hint: filterOwners.find((owner) => owner.value === row.owner)?.name ?? '',
                }));
      const selected = q.get('selected');
      const search = q.get('search')?.toLowerCase() ?? '';
      const filtered = choices.filter((row) =>
        selected ? row.value === selected : row.name.toLowerCase().includes(search),
      );
      const offset = Number(q.get('cursor') ?? 0);
      const limit = Math.min(50, Number(q.get('limit') ?? 50));
      const visible = filtered.slice(offset, offset + limit);
      return route.fulfill({
        json: {
          items: visible.map(({ value, label, hint }) => ({
            value,
            label,
            ...(visible.filter((other) => other.label === label).length > 1
              ? { disambiguator: hint }
              : {}),
          })),
          next: filtered.length > offset + limit ? String(offset + limit) : null,
        },
      });
    });
  }
}
