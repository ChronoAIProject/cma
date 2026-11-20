import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { type UsageMetricPage, type UsageMetricQuantity } from '@cma/client';
import { AllocationChart, allocationComparisons } from './AllocationChart';
import { AllocationHistory, humanDuration } from './AllocationHistory';
import { UsageIdentity } from './UsageIdentity';
import { asOf, metricRow, metricsClient } from './testFixtures';

const chartProps = {
  api: metricsClient(true),
  reportId: 'fixture-report',
  active: true,
  onFailure: vi.fn(),
};

const page: UsageMetricPage = {
  report_id: 'fixture-report',
  as_of: asOf,
  next: 'next-page',
  total_count: 200,
  items: [
    metricRow,
    {
      ...metricRow,
      sandbox_id: 'sbx-2',
      name: 'Build Sandbox',
      allocated_period: {
        ...metricRow.allocated_period,
        cpu_core_hours: 12,
        memory_gib_hours: 24,
        complete: true,
      },
    },
  ],
};

it('compares page allocation with distinct units, explicit scope and keyboard-accessible data', async () => {
  const actor = userEvent.setup();
  render(<AllocationChart {...chartProps} page={page} admin />);
  expect(screen.getByRole('img')).toHaveAccessibleName('CPU allocated core-hours by sandbox');
  expect(screen.getByText(/2 of 200 Sandboxes on this page/)).toBeInTheDocument();
  const disclosure = screen.getByText('View allocation chart data');
  await actor.click(disclosure);
  const table = await screen.findByRole('table', { name: 'Allocation chart data' });
  expect(within(table).getByRole('cell', { name: '24' })).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Allocation chart resource' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Memory' }));
  expect(screen.getByRole('img')).toHaveAccessibleName('Memory allocated GiB-hours by sandbox');
  expect(within(table).getByRole('cell', { name: '48' })).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Allocation chart grouping' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'By owner' }));
  expect(
    await screen.findByRole('region', { name: 'Scrollable owner allocations' }),
  ).toBeInTheDocument();
  expect(chartProps.api.ownerAllocations).toHaveBeenCalledWith(
    'fixture-report',
    expect.objectContaining({ metric: 'memory' }),
  );
});

it('keeps all page groups in the fallback and never turns an empty page into zero bars', async () => {
  const actor = userEvent.setup();
  const many = {
    ...page,
    items: Array.from({ length: 10 }, (_, index) => ({
      ...metricRow,
      sandbox_id: `sandbox-${index}`,
      name: `Sandbox ${index}`,
    })),
  };
  const view = render(<AllocationChart {...chartProps} page={many} admin={false} />);
  expect(screen.getByText(/Largest 8 shown/)).toBeInTheDocument();
  await actor.click(screen.getByText('View allocation chart data'));
  expect(
    within(await screen.findByRole('table', { name: 'Allocation chart data' })).getAllByRole(
      'row',
      { hidden: true },
    ),
  ).toHaveLength(11);
  view.rerender(<AllocationChart {...chartProps} page={{ ...page, items: [] }} admin={false} />);
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(screen.queryByText(/0 core-hours/)).not.toBeInTheDocument();
});

it('keeps distinct Sandbox identities when their names match', () => {
  const rows = [
    metricRow,
    { ...metricRow, sandbox_id: 'other', owner: { ...metricRow.owner, subject: 'another-owner' } },
  ];
  expect(allocationComparisons(rows, 'cpu')).toHaveLength(2);
});

it('explains incomplete history without exposing raw seconds or implying missing time is zero', async () => {
  const actor = userEvent.setup();
  render(<AllocationHistory value={metricRow.allocated_period} />);
  await actor.click(screen.getByText('Some allocation history is missing'));
  expect(screen.getByText('1h')).toBeVisible();
  expect(screen.getByText('30s')).toBeVisible();
  expect(screen.getByText(/Missing history is excluded.*does not mean zero/)).toBeVisible();
  expect(humanDuration(121779)).toBe('1d 9h');
  expect(humanDuration(15667207)).toBe('181d 8h');
  expect(humanDuration('9007199254740993')).toContain('104,249,991,374d');
});

it('distinguishes projected time from missing history in the default daily report', async () => {
  const actor = userEvent.setup();
  render(
    <AllocationHistory
      value={{
        ...metricRow.allocated_period,
        complete: false,
        unknown_seconds: 0,
        projected_tail_seconds: 173040,
      }}
    />,
  );
  expect(screen.queryByText('Some allocation history is missing')).not.toBeInTheDocument();
  expect(screen.queryByText('What’s missing?')).not.toBeInTheDocument();
  expect(screen.getByText('Coverage unverified')).toBeVisible();
  await actor.click(screen.getByText('Includes time extended from last known state'));
  expect(screen.getByText('Missing history').nextElementSibling).toHaveTextContent('None');
  expect(screen.getByText('Extended from last known state').nextElementSibling).toHaveTextContent(
    '2d 4m',
  );
  expect(screen.getByText(/Allocation history coverage is not fully verified/)).toBeVisible();
  expect(screen.getByText(/Durations add up across Sandboxes/)).toBeVisible();
});

const quantities: {
  kind: string;
  zero: UsageMetricQuantity;
  positive: UsageMetricQuantity;
  duration: string;
}[] = [
  { kind: 'numbers', zero: 0, positive: 3600, duration: '1h' },
  { kind: 'decimal strings', zero: '0', positive: '3600', duration: '1h' },
  {
    kind: 'large integer strings',
    zero: '0',
    positive: '9007199254740993',
    duration: '104,249,991,374d 7h',
  },
];

for (const { kind, zero, positive, duration } of quantities) {
  it.each([
    [false, false, false, 'Coverage unverified'],
    [false, false, true, 'Coverage unverified'],
    [false, true, false, 'Partial history'],
    [false, true, true, 'Partial history'],
    [true, false, false, 'Complete history'],
    [true, false, true, 'Complete history'],
    [true, true, false, 'Partial history'],
    [true, true, true, 'Partial history'],
  ] as const)(
    `${kind}: complete=%s, missing=%s, projected=%s displays %s`,
    async (complete, missing, projected, badge) => {
      const actor = userEvent.setup();
      const value = Object.freeze({
        ...metricRow.allocated_period,
        complete,
        unknown_seconds: missing ? positive : zero,
        projected_tail_seconds: projected ? positive : zero,
      });
      render(<AllocationHistory value={value} />);
      expect(screen.getByText(badge)).toBeVisible();
      if (badge !== 'Complete history')
        expect(screen.queryByText('Complete history')).not.toBeInTheDocument();
      expect(screen.queryByText('Some allocation history is missing') !== null).toBe(missing);
      expect(screen.queryByText('Includes time extended from last known state') !== null).toBe(
        !missing && projected,
      );
      await actor.click(screen.getByText(missing ? 'What’s missing?' : 'Details'));
      expect(screen.getByText('Missing history').nextElementSibling).toHaveTextContent(
        missing ? duration : 'None',
      );
      expect(
        screen.getByText('Extended from last known state').nextElementSibling,
      ).toHaveTextContent(projected ? duration : 'None');
      expect(screen.queryByText(/Missing history is excluded.*does not mean zero/) !== null).toBe(
        missing,
      );
      expect(screen.queryByText(/This report does not identify the reason/) !== null).toBe(
        badge === 'Coverage unverified',
      );
      expect(screen.queryByText(/^The last known allocation is extended/) !== null).toBe(projected);
      expect(value.complete).toBe(complete);
    },
  );
}

it('updates the summary and expanded facts together when report coverage changes', async () => {
  const actor = userEvent.setup();
  const view = render(<AllocationHistory value={metricRow.allocated_period} />);
  await actor.click(screen.getByText('What’s missing?'));
  const period = { ...metricRow.allocated_period, unknown_seconds: '0' };
  view.rerender(<AllocationHistory value={period} />);
  expect(screen.getByText('Coverage unverified')).toBeVisible();
  expect(screen.getByText('Includes time extended from last known state')).toBeVisible();
  expect(screen.getByText('Missing history').nextElementSibling).toHaveTextContent('None');
  expect(screen.queryByText('What’s missing?')).not.toBeInTheDocument();
  view.rerender(
    <AllocationHistory value={{ ...period, complete: true, projected_tail_seconds: '0' }} />,
  );
  expect(screen.getByText('Complete history')).toBeVisible();
  expect(screen.getByText('How allocation is calculated')).toBeVisible();
  expect(screen.queryByText('Coverage unverified')).not.toBeInTheDocument();
  expect(screen.queryByText(/^The last known allocation is extended/)).not.toBeInTheDocument();
  expect(screen.getByText('Extended from last known state').nextElementSibling).toHaveTextContent(
    'None',
  );
  view.rerender(<AllocationHistory value={metricRow.allocated_period} />);
  expect(screen.getByText('Partial history')).toBeVisible();
  expect(screen.getByText('Some allocation history is missing')).toBeVisible();
  expect(screen.getByText('Missing history').nextElementSibling).toHaveTextContent('1h');
  expect(screen.queryByText('Complete history')).not.toBeInTheDocument();
});

it('copies the complete identity and shows a selectable fallback when clipboard access fails', async () => {
  const actor = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const value = `registry.example.test/runtime@sha256:${'a'.repeat(64)}`;
  render(<UsageIdentity label="runtime image" value={value} />);
  expect(screen.getByTitle(value)).toHaveTextContent('…');
  await actor.click(screen.getByRole('button', { name: 'Copy runtime image' }));
  expect(copy).toHaveBeenCalledWith(value);
  expect(await screen.findByRole('status')).toHaveTextContent('runtime image copied');
  copy.mockRejectedValue(new Error('Fixture clipboard refusal'));
  await actor.click(screen.getByRole('button', { name: 'Copy runtime image' }));
  expect(await screen.findByText(value, { exact: true })).toBeVisible();
  copy.mockRestore();
});
