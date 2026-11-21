import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { NodeFields } from './NodeFields';
import { defaultSource } from './model';
import { validateRelayAuthoring } from './relayAuthoring';

const discovery: RelayDiscovery = {
  status: 'ready',
  authoring: {
    native_discord_writes_enabled: true,
    native_discord_auto_activation_enabled: true,
    response_recovery_providers: ['cma_codex'],
    multi_consumer_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const native = { ...defaultSource('discord'), bot_id: '77' };
const draft = { sources: [native], provider: { slug: 'nyxbot' } };

it('explains a provider refusal before saving and updates when the provider changes', () => {
  const props = {
    onChange: vi.fn(),
    credentials: {},
    onCredentials: vi.fn(),
    section: { kind: 'channel' as const },
    create: true,
    sourceCount: 0,
    denied: vi.fn(),
  };
  const view = render(
    <RelayCapabilitiesContext value={discovery}>
      <NodeFields {...props} draft={draft} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByText(/nyxbot does not support reconnecting/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  const choice = screen.getByRole('menuitemradio', { name: /Direct Discord bot/ });
  expect(choice).toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(choice);
  expect(props.onChange).not.toHaveBeenCalled();
  fireEvent.keyDown(document, { key: 'Escape' });
  view.rerender(
    <RelayCapabilitiesContext value={discovery}>
      <NodeFields {...props} draft={{ ...draft, provider: { slug: 'cma_codex' } }} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.queryByText(/nyxbot does not support reconnecting/)).not.toBeInTheDocument();
});

it('blocks a new unsupported source but preserves existing channels and relay definitions', () => {
  expect(() =>
    validateRelayAuthoring(discovery, draft, undefined, {
      discord_bot_token: 'test-only-token',
    }),
  ).toThrow('nyxbot does not support reconnecting');
  expect(() => validateRelayAuthoring(discovery, draft, draft)).not.toThrow();
  expect(() =>
    validateRelayAuthoring(discovery, {
      ...draft,
      sources: [defaultSource('custom')],
    }),
  ).not.toThrow();
  const enabled = {
    ...discovery,
    authoring: { ...discovery.authoring!, response_recovery_providers: ['cma_codex', 'nyxbot'] },
  };
  expect(() =>
    validateRelayAuthoring(enabled, draft, undefined, {
      discord_bot_token: 'test-only-token',
    }),
  ).not.toThrow();
});

it('preserves compatibility when an older gateway omits the additive discovery field', () => {
  const older = {
    ...discovery,
    authoring: { ...discovery.authoring!, response_recovery_providers: undefined },
  };
  expect(() =>
    validateRelayAuthoring(older, draft, undefined, {
      discord_bot_token: 'test-only-token',
    }),
  ).not.toThrow();
});
