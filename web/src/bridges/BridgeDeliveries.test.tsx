import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import corpus from '../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-outbox.json';
import { BridgeDeliveries } from './BridgeDeliveries';
const sample = (name: string) =>
  corpus.cases.find((value) => value.id === `bridge-outbox-${name}`)!.response.body;
const bridge = 'brg_f62e916c2d1db698835e7bbb1f1ad9c97b43d3f53f010303582719e84468b235';
function fixture() {
  const writes: { body: unknown; key: string | null }[] = [];
  const state = { uncertain: false, revoked: false };
  const fetch = vi.fn(
    async (input: RequestInfo | URL, options?: RequestInit): Promise<Response> => {
      const path = new URL(String(input)).pathname;
      if (state.revoked)
        return Response.json(
          { code: 'bridge_not_found', detail: 'No current access.' },
          { status: 404 },
        );
      if (path.endsWith('/deliveries')) return Response.json(sample('list-unknown'));
      expect(path).toBe(`/api/v1/bridges/${bridge}/delivery-recovery`);
      writes.push({
        body: JSON.parse(String(options?.body)),
        key: new Headers(options?.headers).get('Idempotency-Key'),
      });
      if (state.uncertain) {
        state.uncertain = false;
        throw new TypeError('response lost');
      }
      const body = JSON.parse(String(options?.body)) as { action: { kind: string } };
      return Response.json(
        sample(body.action.kind === 'reconcile' ? 'reconcile-unknown' : 'retry-explicit'),
      );
    },
  );
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const changed = vi.fn();
  const props = { client, id: bridge, revision: 2, allowed: true, onAccessChanged: changed };
  return { fetch, state, writes, props, changed };
}
it('requires duplicate-risk acceptance and preserves exact uncertain recovery across history refresh', async () => {
  const test = fixture();
  test.state.uncertain = true;
  render(<BridgeDeliveries {...test.props} />);
  const user = userEvent.setup();
  const retry = await screen.findByRole('button', { name: 'Retry original reply' });
  expect(retry).toBeDisabled();
  await user.click(screen.getByRole('checkbox', { name: /I accept that retrying/ }));
  await user.click(retry);
  await screen.findByRole('button', { name: 'Recover previous action' });
  expect(test.writes).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Refresh replies' }));
  await screen.findByText(/The reply may have reached/);
  await user.click(screen.getByRole('button', { name: 'Recover previous action' }));
  await screen.findByText('The original reply is queued again. Delivery has not been confirmed.');
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]).toEqual(test.writes[0]);
  expect(JSON.stringify(test.writes)).toContain('"accept_duplicate_risk":true');
  expect(JSON.stringify(test.writes)).not.toContain('chat_id');
});
it('keeps unconfirmed sends unknown when refreshing local attempt state', async () => {
  const test = fixture();
  render(<BridgeDeliveries {...test.props} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Check delivery state' }));
  await screen.findByText('Delivery state checked: Delivery unknown.');
  expect(screen.getByText(/The connector cannot verify this earlier send/)).toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});
it('fetches no manager history for viewers and clears rows on access refusal', async () => {
  const test = fixture();
  const view = render(<BridgeDeliveries {...test.props} allowed={false} />);
  await act(async () => {});
  expect(test.fetch).not.toHaveBeenCalled();
  view.rerender(<BridgeDeliveries {...test.props} />);
  await screen.findByText(/The reply may have reached/);
  test.state.revoked = true;
  await userEvent.setup().click(screen.getByRole('button', { name: 'Refresh replies' }));
  await waitFor(() => expect(test.changed).toHaveBeenCalled());
  expect(screen.queryByRole('button', { name: 'Retry original reply' })).not.toBeInTheDocument();
  expect(screen.queryByText(/The reply may have reached/)).not.toBeInTheDocument();
});
