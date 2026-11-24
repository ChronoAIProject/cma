import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { MetricFilters } from './MetricFilters';
import { OpsMetricFilters } from './OpsMetricFilters';
import { MetricsNotice } from './MetricsNotice';
import { defaultSelection, type Selection } from './model';
import { metricsClient } from './testFixtures';

const rangeError = 'Choose a valid range of up to 30 days, ending no later than now.';
const pendingNotice = 'Changes apply when you select Apply filters.';
const reversed: Selection = {
  ...defaultSelection,
  range: 'custom',
  from: '2020-01-03T00:00:00.000Z',
  to: '2020-01-01T00:00:00.000Z',
};
async function chooseRange(name: string) {
  await userEvent.click(screen.getByRole('button', { name: 'Usage time range' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name }));
}
const submit = () => userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));

describe.each(['owner', 'operations'] as const)('%s range validation', (surface) => {
  function setup(selection = defaultSelection) {
    const api = metricsClient(surface === 'operations');
    const onApply = vi.fn();
    const view = (value: Selection) =>
      surface === 'owner' ? (
        <MetricFilters api={api} selection={value} admin={false} active onApply={onApply} />
      ) : (
        <OpsMetricFilters api={api} selection={value} active onApply={onApply} />
      );
    return { ...render(view(selection)), api, onApply, view };
  }

  it.each(['Last hour', 'Last 24 hours', 'Last 7 days', 'Last 30 days'])(
    'clears invalid Custom feedback when choosing %s without applying',
    async (preset) => {
      const f = setup();
      expect(screen.queryByText(pendingNotice)).not.toBeInTheDocument();
      await chooseRange('Custom (up to 30 days)');
      fireEvent.change(screen.getByLabelText('Usage range from'), {
        target: { value: '2020-01-03T00:00' },
      });
      fireEvent.change(screen.getByLabelText('Usage range to'), {
        target: { value: '2020-01-01T00:00' },
      });
      await submit();
      expect(screen.getByRole('alert')).toHaveTextContent(rangeError);
      await chooseRange(preset);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Usage time range' })).toHaveTextContent(preset);
      expect(screen.getByText(pendingNotice)).toBeVisible();
      expect(f.onApply).not.toHaveBeenCalled();
      expect(f.api.create).not.toHaveBeenCalled();
      await chooseRange('Custom (up to 30 days)');
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      await submit();
      expect(screen.getByRole('alert')).toHaveTextContent(rangeError);
      expect(f.onApply).not.toHaveBeenCalled();
      fireEvent.change(screen.getByLabelText('Usage range to'), {
        target: { value: '2020-01-04T00:00' },
      });
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      await submit();
      expect(f.onApply).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ range: 'custom' }),
      );
      const applied = f.onApply.mock.calls[0]![0] as Selection;
      expect(Date.parse(applied.to)).toBeGreaterThan(Date.parse(applied.from));
      f.rerender(f.view(applied));
      expect(screen.queryByText(pendingNotice)).not.toBeInTheDocument();
    },
  );

  it('preserves errors across unrelated renders and edits, then resets on external selection changes', async () => {
    const f = setup(reversed);
    await submit();
    f.rerender(f.view(reversed));
    expect(screen.getByRole('alert')).toHaveTextContent(rangeError);
    if (surface === 'owner')
      fireEvent.change(screen.getByRole('searchbox', { name: 'Sandbox name or ID starts with' }), {
        target: { value: 'research' },
      });
    else {
      await userEvent.click(screen.getByRole('button', { name: 'Usage Sandbox status' }));
      await userEvent.click(screen.getByRole('menuitemradio', { name: 'Live' }));
    }
    expect(screen.getByRole('alert')).toHaveTextContent(rangeError);
    await submit();
    expect(f.onApply).not.toHaveBeenCalled();
    f.rerender(f.view({ ...defaultSelection, range: '7d' }));
    expect(screen.getByRole('button', { name: 'Usage time range' })).toHaveTextContent(
      'Last 7 days',
    );
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText(pendingNotice)).not.toBeInTheDocument();
    expect(f.onApply).not.toHaveBeenCalled();
  });

  it.each([
    ['empty', '', '2020-01-02T00:00:00.000Z'],
    ['future', '2099-01-01T00:00:00.000Z', '2099-01-02T00:00:00.000Z'],
    ['over 30 days', '2020-01-01T00:00:00.000Z', '2020-03-01T00:00:00.000Z'],
    ['unparseable', 'invalid', '2020-01-02T00:00:00.000Z'],
  ])('still refuses %s ranges at the submit boundary', async (_, from, to) => {
    const f = setup({ ...defaultSelection, range: 'custom', from: from!, to: to! });
    const form = screen.getByRole('form', { name: 'Usage filters' });
    // Exercise the submit boundary as well as the native required-input constraint.
    fireEvent.submit(form);
    expect(screen.getByRole('alert')).toHaveTextContent(rangeError);
    expect(f.onApply).not.toHaveBeenCalled();
  });

  it('range edits clear only local validation and retain independent request failures', async () => {
    const f = setup(reversed);
    render(
      <MetricsNotice
        failure={{
          title: 'Usage unavailable',
          message: 'Report request failed.',
          denied: false,
          expired: false,
        }}
        onRetry={vi.fn()}
      />,
    );
    await submit();
    const form = screen.getByRole('form', { name: 'Usage filters' });
    expect(within(form).getByRole('alert')).toHaveTextContent(rangeError);
    fireEvent.change(screen.getByLabelText('Usage range from'), {
      target: { value: '2019-12-31T00:00' },
    });
    expect(within(form).queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByText('Report request failed.')).toBeVisible();
    expect(f.onApply).not.toHaveBeenCalled();
  });
});
