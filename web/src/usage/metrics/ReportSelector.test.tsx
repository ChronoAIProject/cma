import { useState } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { ReportSelector } from './ReportSelector';
import { defaultSelection } from './model';
import { metricsClient, metricRow, reportFixture } from './testFixtures';

it('retains the chosen Sandbox name after closing and reopening discovery', async () => {
  const api = metricsClient();
  function Subject() {
    const [value, onChange] = useState('');
    return (
      <ReportSelector
        api={api}
        selection={defaultSelection}
        kind="sandbox"
        value={value}
        onChange={onChange}
        active
        admin={false}
      />
    );
  }
  render(<Subject />);
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: All my Sandboxes' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Choose sandbox' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research Sandbox (sbx-1)' }));
  const selected = screen.getByRole('button', { name: 'Sandbox: Research Sandbox' });
  expect(selected).toHaveTextContent('Research Sandbox');
  expect(selected.querySelector('[title]')).toHaveAttribute('title', 'Research Sandbox');
  await userEvent.click(selected);
  expect(await screen.findByRole('button', { name: 'Choose sandbox' })).toHaveTextContent(
    'Research Sandbox',
  );
  expect(api.sandbox).not.toHaveBeenCalled();
  expect(api.options).not.toHaveBeenCalled();
});

function selectedReport() {
  const report = reportFixture();
  return {
    ...report,
    query: { ...report.query, filters: { ...report.query.filters, sandbox_id: 'sbx-1' } },
  };
}

it('restores a URL-only Sandbox from one exact authorized report read', async () => {
  const api = metricsClient();
  let finish!: (value: typeof metricRow) => void;
  api.sandbox.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const change = vi.fn();
  const props = {
    api,
    selection: { ...defaultSelection, sandbox: 'sbx-1' },
    kind: 'sandbox' as const,
    value: 'sbx-1',
    onChange: change,
    active: true,
    admin: false,
  };
  const { rerender } = render(<ReportSelector {...props} />);
  expect(screen.getByRole('button', { name: 'Sandbox: Loading Sandbox…' })).toBeVisible();
  expect(api.sandbox).not.toHaveBeenCalled();
  rerender(<ReportSelector {...props} selectedReport={selectedReport()} />);
  await waitFor(() =>
    expect(api.sandbox).toHaveBeenCalledWith(
      'report-one',
      'sbx-1',
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    ),
  );
  await act(async () => finish({ ...metricRow, name: 'A long Sandbox name '.repeat(10).trim() }));
  const button = screen.getByRole('button', {
    name: `Sandbox: ${'A long Sandbox name '.repeat(10).trim()}`,
  });
  expect(button.querySelector('[title]')).toHaveAttribute(
    'title',
    'A long Sandbox name '.repeat(10).trim(),
  );
  rerender(<ReportSelector {...props} selectedReport={selectedReport()} />);
  expect(api.sandbox).toHaveBeenCalledOnce();
  expect(api.create).not.toHaveBeenCalled();
  expect(api.sandboxes).not.toHaveBeenCalled();
  expect(api.options).not.toHaveBeenCalled();
  expect(change).not.toHaveBeenCalled();
});

it.each([
  new ApiError(403, 'Forbidden'),
  new ApiError(404, 'Missing'),
  { ...metricRow, sandbox_id: 'wrong', name: 'Other private name' },
  {
    ...metricRow,
    owner: { subject: 'other-owner', display_name: null },
    name: 'Other private name',
  },
])('keeps an unavailable Sandbox selected and offers bounded recovery: %j', async (result) => {
  const api = metricsClient();
  if (result instanceof Error) api.sandbox.mockRejectedValue(result);
  else api.sandbox.mockResolvedValue(result);
  const change = vi.fn();
  const props = {
    api,
    selection: defaultSelection,
    kind: 'sandbox' as const,
    value: 'sbx-1',
    onChange: change,
    active: true,
    admin: false,
    selectedReport: selectedReport(),
  };
  const { rerender } = render(<ReportSelector {...props} />);
  await screen.findByRole('button', { name: 'Sandbox: Selected Sandbox unavailable' });
  expect(screen.queryByText('Other private name')).not.toBeInTheDocument();
  rerender(<ReportSelector {...props} active={false} />);
  rerender(<ReportSelector {...props} />);
  expect(api.sandbox).toHaveBeenCalledOnce();
  expect(change).not.toHaveBeenCalled();
  api.sandbox.mockResolvedValue({ ...metricRow, name: ' ' });
  await userEvent.click(screen.getByRole('button', { name: 'Retry Sandbox name' }));
  await screen.findByRole('button', { name: 'Sandbox: Unnamed Sandbox' });
  expect(api.sandbox).toHaveBeenCalledTimes(2);
  await userEvent.click(screen.getByRole('button', { name: 'Clear Sandbox' }));
  expect(change).toHaveBeenCalledWith('');
});

it('retains same-name and unnamed Sandbox choices across pages without detail reads', async () => {
  const api = metricsClient();
  api.sandboxes.mockImplementation(async (_report, { cursor }) => ({
    report_id: 'report-one',
    as_of: metricRow.last_activity_at,
    items: cursor
      ? [{ ...metricRow, sandbox_id: 'sbx-later', name: '' }]
      : [
          { ...metricRow, sandbox_id: 'sbx-a' },
          { ...metricRow, sandbox_id: 'sbx-b' },
        ],
    next: cursor ? null : 'next-page',
  }));
  function Subject() {
    const [value, onChange] = useState('');
    return (
      <ReportSelector
        api={api}
        selection={defaultSelection}
        kind="sandbox"
        value={value}
        onChange={onChange}
        active
        admin={false}
      />
    );
  }
  render(<Subject />);
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: All my Sandboxes' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Choose sandbox' }));
  expect(screen.getByRole('menuitemradio', { name: 'Research Sandbox (sbx-a)' })).toBeVisible();
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research Sandbox (sbx-b)' }));
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: Research Sandbox' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Next choices' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next choices' })).toBeDisabled());
  expect(screen.getByRole('button', { name: 'Choose sandbox' })).toHaveTextContent(
    'Research Sandbox',
  );
  await userEvent.click(screen.getByRole('button', { name: 'Choose sandbox' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Unnamed Sandbox (sbx-later)' }));
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: Unnamed Sandbox' }));
  expect(await screen.findByRole('button', { name: 'Choose sandbox' })).toHaveTextContent(
    'Unnamed Sandbox',
  );
  expect(api.sandbox).not.toHaveBeenCalled();
});

it('searches all owner subjects and pages their frozen cohort without scanning Sandboxes', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  const change = vi.fn();
  render(
    <ReportSelector
      api={api}
      selection={defaultSelection}
      kind="owner"
      value=""
      onChange={change}
      active
      admin
    />,
  );
  expect(api.create).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await screen.findByRole('button', { name: 'Choose owner' });
  expect(api.owners).toHaveBeenCalledWith(
    'report-one',
    expect.objectContaining({ limit: 50, search: '' }),
  );
  await actor.type(screen.getByRole('textbox', { name: 'Search owner choices' }), 'remote-owner');
  api.owners.mockResolvedValue({
    report_id: 'report-one',
    items: [{ subject: 'remote-owner-25001', display_name: 'Remote owner' }],
    next: 'next-owners',
  });
  await actor.click(screen.getByRole('button', { name: 'Search owner choices' }));
  await waitFor(() =>
    expect(api.owners).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ search: 'remote-owner', cursor: undefined, limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Next choices' }));
  await waitFor(() =>
    expect(api.owners).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ search: 'remote-owner', cursor: 'next-owners', limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Choose owner' }));
  await actor.click(
    screen.getByRole('menuitemradio', { name: 'Remote owner (remote-owner-25001)' }),
  );
  expect(change).toHaveBeenCalledWith('remote-owner-25001');
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.sandboxes).not.toHaveBeenCalled();
});
it('distinguishes failed choices from empty choices and retries without changing the filter', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  api.owners.mockRejectedValueOnce(new ApiError(400, 'Invalid selection'));
  const change = vi.fn();
  render(
    <ReportSelector
      api={api}
      selection={defaultSelection}
      kind="owner"
      value="owner-1"
      onChange={change}
      active
      admin
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Owner: owner-1' }));
  await screen.findByRole('alert');
  expect(screen.queryByText('No matching owner choices.')).not.toBeInTheDocument();
  api.owners.mockResolvedValue({ report_id: 'report-one', items: [], next: null });
  await actor.click(screen.getByRole('button', { name: 'Retry usage' }));
  await screen.findByText('No matching owner choices.');
  expect(change).not.toHaveBeenCalled();
});
