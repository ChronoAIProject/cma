import { render, screen, waitFor, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { BridgeCapabilities } from './BridgeCapabilities';
import { bridgeTransport } from './fixtures.test-support';

const connector = {
  platform: 'telegram',
  evidence: 'contract_tested',
  live_verified: false,
  inbound_text: true,
  stable_sender_chat: true,
  stable_thread: 'telegram_forum_topic',
  reply_to_bot: true,
  bot_mentions: false,
  inbound_edits: false,
  inbound_deletes: false,
  verified_actions: false,
  attachment_input: 'download_required',
  attachment_output: false,
  output_text: true,
  output_edit: false,
  output_progress: true,
  output_limit_utf16: 4000,
  output_format: 'escaped_markdown',
  stages: { management: true, ingress: true, allocation: false, execution: false, outbound: true },
};
const catalog = {
  version: 1,
  source_revision: '85607255ee0beadca460a350e0aa6d177edd2fd2',
  connection_validation_available: true,
  connectors: [connector],
};
it('shows actual independent stage switches and distinguishes fixtures from live evidence', async () => {
  const test = bridgeTransport((url) =>
    url.pathname.endsWith('/capabilities') ? Response.json(catalog) : undefined,
  );
  render(<BridgeCapabilities client={test.client} platform="telegram" organization="org-one" />);
  await screen.findByText(/Live platform delivery has not been verified/);
  expect(
    screen.getByText(/Messages with attachments need a download capability/),
  ).toHaveTextContent('Send text separately.');
  expect(screen.getByText('Message intake').nextElementSibling).toHaveTextContent('Enabled');
  expect(screen.getByText('Agent execution').nextElementSibling).toHaveTextContent('Disabled');
  expect(screen.getByText('Channel replies').nextElementSibling).toHaveTextContent('Enabled');
  expect(String(test.fetch.mock.calls[0]![0])).toContain('organization=org-one');
});
it('clears a prior capability projection immediately when the owner scope changes', async () => {
  const test = bridgeTransport((url) =>
    url.pathname.endsWith('/capabilities')
      ? url.searchParams.has('organization')
        ? new Response(null, { status: 403 })
        : Response.json(catalog)
      : undefined,
  );
  const view = render(<BridgeCapabilities client={test.client} platform="telegram" />);
  await screen.findByText(/Text messages are supported/);
  view.rerender(
    <BridgeCapabilities client={test.client} platform="telegram" organization="other-org" />,
  );
  expect(screen.queryByText(/Text messages are supported/)).not.toBeInTheDocument();
  await waitFor(() =>
    expect(
      within(screen.getByRole('region', { name: 'Connector capabilities' })).getByText(
        /Connector capabilities are unavailable/,
      ),
    ).toBeInTheDocument(),
  );
});
it('treats an old API or unknown platform as unavailable without guessing support', async () => {
  const test = bridgeTransport((url) =>
    url.pathname.endsWith('/capabilities') ? new Response(null, { status: 404 }) : undefined,
  );
  render(<BridgeCapabilities client={test.client} platform="telegram" />);
  await screen.findByText(/Connector capabilities are unavailable/);
  expect(screen.queryByText(/Text messages are supported/)).not.toBeInTheDocument();
});
