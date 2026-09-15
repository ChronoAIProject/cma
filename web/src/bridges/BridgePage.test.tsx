import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { BridgePage } from './BridgePage';
import { BridgeCredential } from './BridgeCredential';
import { BridgeManager } from './BridgeManager';
import { bridgeDetail, bridgeTransport } from './fixtures.test-support';
import { chooseOption } from '../test/select';
import { NOW } from '../test/rest';

describe('bridge management UI', () => {
  it('shows scoped inventory and clears protected rows on owner or viewer change', async () => {
    let finish: ((response: Response) => void) | undefined;
    const test = bridgeTransport((url) =>
      url.searchParams.has('organization')
        ? new Promise<Response>((resolve) => {
            finish = resolve;
          })
        : undefined,
    );
    const navigate = vi.fn();
    const view = render(
      <BridgePage
        client={test.client}
        viewerId="owner"
        route={{ kind: 'bridges' }}
        onNavigate={navigate}
      />,
    );
    await screen.findByRole('link', { name: 'Client support' });
    const user = userEvent.setup();
    await chooseOption(user, 'Bridge owner scope', 'Research');
    expect(navigate).toHaveBeenCalledWith({ kind: 'bridges', organization: 'org-one' });
    view.rerender(
      <BridgePage
        client={test.client}
        viewerId="owner"
        route={{ kind: 'bridges', organization: 'org-one' }}
        onNavigate={navigate}
      />,
    );
    expect(screen.queryByText('Client support')).not.toBeInTheDocument();
    await waitFor(() => expect(finish).toBeDefined());
    expect(screen.getByRole('button', { name: 'New bridge' })).toBeDisabled();
    const other = bridgeTransport((url) =>
      url.pathname.endsWith('/bridges') ? Response.json({ items: [] }) : undefined,
    );
    view.rerender(
      <BridgePage
        client={other.client}
        viewerId="other"
        route={{ kind: 'bridges' }}
        onNavigate={navigate}
      />,
    );
    await screen.findByText('No bridges in this scope.');
    await act(async () =>
      finish?.(
        Response.json({
          items: [{ ...bridgeDetail.bridge, owner: { kind: 'organization', subject: 'org-one' } }],
        }),
      ),
    );
    expect(screen.queryByText('Client support')).not.toBeInTheDocument();
  });

  it('does not request configuration, key choices or grants for a member detail', async () => {
    const test = bridgeTransport((url) =>
      url.pathname.endsWith('/bridges/brg_one')
        ? Response.json({ ...bridgeDetail.bridge, can_manage: false })
        : undefined,
    );
    render(
      <BridgePage
        client={test.client}
        viewerId="member"
        route={{ kind: 'bridge', bridgeId: 'brg_one' }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByText(/owner or an organization admin can manage/);
    expect(test.fetch.mock.calls.map(([url]) => new URL(String(url)).pathname)).toEqual([
      '/api/v1/bridges/brg_one',
    ]);
    expect(screen.queryByLabelText('Import selected Agent key')).not.toBeInTheDocument();
  });

  it('retains edited settings through access refresh and retries an ambiguous save with the same body and key', async () => {
    let attempts = 0;
    const writes: RequestInit[] = [];
    const test = bridgeTransport((url, init) => {
      if (url.pathname.endsWith('/configuration') && init.method === 'PUT') {
        writes.push(init);
        attempts += 1;
        if (attempts === 1) return Promise.reject(new TypeError('connection lost'));
        return Response.json({
          bridge_id: 'brg_one',
          owner: bridgeDetail.bridge.owner,
          actor: { kind: 'human', subject: 'owner' },
          revision: 2,
          config_revision: 2,
          command: 'update',
          applied_at: NOW,
        });
      }
      return undefined;
    });
    const view = render(
      <BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />,
    );
    const user = userEvent.setup();
    const name = await screen.findByLabelText('Bridge name');
    await waitFor(() => expect(name).toBeEnabled());
    await user.clear(name);
    await user.type(name, 'Retained draft');
    view.rerender(
      <BridgeManager client={test.client} id="brg_one" allowed={false} onChanged={vi.fn()} />,
    );
    expect(screen.queryByLabelText('Bridge name')).not.toBeInTheDocument();
    view.rerender(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
    expect(await screen.findByLabelText('Bridge name')).toHaveValue('Retained draft');
    await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
    await user.click(screen.getByRole('checkbox', { name: /I reviewed resource ownership/ }));
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByRole('button', { name: 'Retry save settings' });
    expect(screen.getByLabelText('Bridge name')).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Retry save settings' }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[0]?.body).toBe(writes[1]?.body);
    expect(new Headers(writes[0]?.headers).get('Idempotency-Key')).toBe(
      new Headers(writes[1]?.headers).get('Idempotency-Key'),
    );
    expect(JSON.parse(String(writes[0]?.body))).toMatchObject({
      expected_revision: 1,
      config: { display_name: 'Retained draft' },
      previous_work: 'suspend_original',
    });
  });

  it('clears a key immediately on submission and never renders a provider error containing it', async () => {
    let finish: ((response: Response) => void) | undefined;
    const test = bridgeTransport((url) =>
      url.pathname.endsWith('/credential')
        ? new Promise<Response>((resolve) => {
            finish = resolve;
          })
        : undefined,
    );
    render(
      <BridgeCredential client={test.client} detail={bridgeDetail} allowed onChanged={vi.fn()} />,
    );
    const user = userEvent.setup();
    const input = screen.getByLabelText('Import selected Agent key');
    await user.type(input, 'fixture-sensitive-material');
    await user.click(screen.getByRole('button', { name: 'Import key' }));
    expect(input).toHaveValue('');
    await waitFor(() => expect(finish).toBeDefined());
    await act(async () =>
      finish?.(
        Response.json(
          { code: 'bridge_binding_mismatch', detail: 'fixture-sensitive-material' },
          { status: 409 },
        ),
      ),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent('verified matching owner');
    expect(document.body.textContent).not.toContain('fixture-sensitive-material');
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });

  it('creates an explicitly reviewed disabled bridge and allows typing multiple sender IDs', async () => {
    let body: Record<string, unknown> | undefined;
    const test = bridgeTransport((url, init) => {
      if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
        body = JSON.parse(String(init.body)) as Record<string, unknown>;
        return Response.json(
          {
            bridge_id: 'brg_created',
            owner: { kind: 'person', subject: 'owner' },
            actor: { kind: 'human', subject: 'owner' },
            revision: 1,
            config_revision: 1,
            command: 'create',
            applied_at: NOW,
          },
          { status: 201 },
        );
      }
      return undefined;
    });
    const navigate = vi.fn();
    render(
      <BridgePage
        client={test.client}
        viewerId="owner"
        route={{ kind: 'bridge_new' }}
        onNavigate={navigate}
      />,
    );
    const user = userEvent.setup();
    await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
    expect(screen.getByRole('button', { name: 'Who shares context' })).toHaveTextContent(
      'Each sender in each chat',
    );
    expect(screen.getByRole('button', { name: 'Allowed senders' })).toHaveTextContent('None');
    await user.type(screen.getByLabelText('Bridge name'), 'New support');
    await chooseOption(user, 'Existing channel bot', 'telegram · bot-one');
    await chooseOption(user, 'Existing channel key', 'key-one');
    await user.click(screen.getByRole('checkbox', { name: 'route-one' }));
    await chooseOption(user, 'Resource placement', 'One retained Agent');
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Retained Agent' })).toBeEnabled(),
    );
    await chooseOption(user, 'Retained Agent', 'Agent one');
    await chooseOption(user, 'Allowed senders', 'Only selected IDs');
    await user.type(screen.getByLabelText('Senders IDs, separated by commas'), '111, 222');
    await user.click(screen.getByRole('checkbox', { name: /I reviewed who owns/ }));
    await user.click(screen.getByRole('button', { name: 'Save disabled bridge' }));
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'bridge', bridgeId: 'brg_created' }),
    );
    expect(body).toMatchObject({
      owner: { kind: 'person', subject: 'owner' },
      config: {
        partition: 'fixed_shared_agent',
        admission: {
          kind: 'scoped',
          policy: { senders: { kind: 'allowlist', ids: ['111', '222'] }, groups: 'deny' },
        },
      },
    });
  });
});
