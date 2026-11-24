import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';

const channel: Channel = {
  channel_id: 'channel_fixture',
  record_id: 'record_fixture',
  version: 7,
  provider: { slug: 'cma', level: 'l1' },
  definition: {
    name: 'Existing channel',
    sources: [],
    reply: { ack_after_ms: 1500 },
    routing: {
      version: 1,
      default_consumer: { name: 'default' },
      consumers: [],
      rules: [],
      default_targets: ['default'],
      max_targets: 4,
      reply_label: 'auto',
      agent_events: { max_hops: 2, per_consumer_per_minute: 10, per_channel_per_minute: 30 },
    },
  },
  endpoints: {
    custom_event_url: 'https://example.test/events',
    nyxid_callback_url: 'https://example.test/callback',
  },
};

const fields = [
  ['Maximum consumers per event', '0', 8],
  ['Maximum handoff hops', '-1', 4],
  ['Events per consumer per minute', '0', 30],
  ['Events per channel per minute', '0', 60],
] as const;

afterEach(() => vi.unstubAllGlobals());

it('identifies invalid routing limits and submits only valid boundary values', async () => {
  const fetcher = vi.fn().mockImplementation(async (_url, init) => {
    const { routing } = JSON.parse(init.body);
    return Response.json({
      ...channel,
      version: 8,
      definition: { ...channel.definition, routing },
    });
  });
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor channel={channel} onDone={vi.fn()} denied={vi.fn()} />);
  const actor = userEvent.setup();
  const save = screen.getByRole('button', { name: 'Save routing' });
  expect(save).toBeEnabled();

  for (const [label, belowMinimum, maximum] of fields) {
    const input = screen.getByRole('spinbutton', { name: label });
    for (const invalid of [belowMinimum, String(maximum + 1), '1.5', '']) {
      fireEvent.change(input, { target: { value: invalid } });
      const error = `Enter a whole number from 1 to ${maximum}.`;
      expect(input).toHaveAttribute('aria-invalid', 'true');
      expect(input).toHaveAccessibleDescription(error);
      expect(screen.getByText(error)).toBeVisible();
      expect(save).toBeDisabled();
      fireEvent.submit(save.closest('form')!);
      expect(fetcher).not.toHaveBeenCalled();
    }
    fireEvent.change(input, { target: { value: '1' } });
    expect(input).not.toHaveAttribute('aria-invalid');
    expect(save).toBeEnabled();
    fireEvent.change(input, { target: { value: String(maximum) } });
    expect(input).not.toHaveAttribute('aria-invalid');
    expect(save).toBeEnabled();
  }

  const acknowledgement = screen.getByRole('spinbutton', { name: 'Acknowledgement delay (ms)' });
  fireEvent.change(acknowledgement, { target: { value: '-1' } });
  expect(acknowledgement).toHaveAttribute('aria-invalid', 'true');
  expect(acknowledgement).toHaveAccessibleDescription(/0.*milliseconds|0.*[0-9]+/);
  expect(save).toBeEnabled();
  await actor.click(save);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
    expected_version: 7,
    routing: {
      max_targets: 8,
      agent_events: { max_hops: 4, per_consumer_per_minute: 30, per_channel_per_minute: 60 },
    },
  });
  await screen.findByText('Changes saved.');
  for (const [label, , maximum] of fields) {
    expect(screen.getByRole('spinbutton', { name: label })).toHaveValue(maximum);
  }
});
