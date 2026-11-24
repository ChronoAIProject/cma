import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { BUILD_LOOKUP_TIMEOUT_MS } from './buildVersions';
import { ProfileMenu, initialsFor } from './ProfileMenu';

vi.mock('../shell/build', async (importOriginal) => {
  const original = await importOriginal<typeof import('../shell/build')>();
  return {
    ...original,
    BUILD_INFO: {
      ...original.BUILD_INFO,
      version: '0.20.22',
      label: '0.20.22',
      revision: 'a'.repeat(40),
      local: false,
    },
  };
});

const amPath = '/api/v1/build-info';
const egPath = '/event-gateway/v1/readyz';
const amBuild = { version: '0.22.34', source_sha: 'b'.repeat(40) };
const egBuild = { version: '0.11.16', source_sha: 'c'.repeat(40) };
const buildLabel = 'Copy build version · Web 0.20.22 · AM 0.22.34 · EG 0.11.16';
const copiedBuilds = `Web 0.20.22 (${'a'.repeat(40)}) · AM 0.22.34 (${'b'.repeat(40)}) · EG 0.11.16 (${'c'.repeat(40)})`;

function mockBuilds() {
  const fetch = vi.fn(async (url: URL) => {
    if (url.pathname === amPath) return Response.json(amBuild);
    if (url.pathname === egPath) {
      return Response.json({ ready: false, build: egBuild }, { status: 503 });
    }
    throw new Error(`Unexpected fetch to ${url}`);
  });
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

const user = {
  id: 'subject-a',
  handle: 'ada',
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  capabilities: [],
  visitor: false,
} as const;

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('account menu', () => {
  it('opens the export inventory only when the owner session advertises export', async () => {
    const fetch = vi.fn(async () =>
      Response.json({
        owner_subject: user.id,
        data_class: 'retained_conversation',
        agents: [],
        next: null,
      }),
    );
    vi.stubGlobal('fetch', (input: URL | string) =>
      input instanceof URL && [amPath, egPath].includes(input.pathname)
        ? Promise.resolve(new Response(null, { status: 404 }))
        : fetch(),
    );
    const actor = userEvent.setup();
    const { rerender } = render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: 'Export account data' })).toBeNull();
    expect(fetch).not.toHaveBeenCalled();
    rerender(<ProfileMenu user={{ ...user, privacyExport: true }} onSignOut={vi.fn()} />);
    await actor.click(screen.getByRole('menuitem', { name: 'Export account data' }));
    expect(await screen.findByText('No Agents to export.')).toBeVisible();
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('opens the published terms and privacy notice in a new tab', async () => {
    const open = vi.fn();
    vi.stubGlobal('open', open);
    const actor = userEvent.setup();
    const { rerender } = render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.queryByRole('menuitem', { name: 'Terms of Service' })).toBeNull();
    rerender(
      <ProfileMenu
        user={{
          ...user,
          legalTerms: {
            version: '2026-11-30',
            termsUrl: 'https://legal.example.test/terms',
            privacyUrl: 'https://legal.example.test/privacy',
          },
        }}
        onSignOut={vi.fn()}
      />,
    );
    await actor.click(screen.getByRole('menuitem', { name: 'Privacy Notice' }));
    expect(open).toHaveBeenCalledWith(
      'https://legal.example.test/privacy',
      '_blank',
      'noopener,noreferrer',
    );
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Terms of Service' }));
    expect(open).toHaveBeenLastCalledWith(
      'https://legal.example.test/terms',
      '_blank',
      'noopener,noreferrer',
    );
  });
  it('keeps a management credential refusal separate from the grant and does not loop sign-in', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        Response.json({
          owner: user.id,
          status: 'unknown',
          management_access: 'unsupported_credential',
        }),
      ),
    );
    const actor = userEvent.setup();
    render(<ProfileMenu user={{ ...user, serviceAuthorization: true }} onSignOut={vi.fn()} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Service authorization' }));
    await screen.findByText(/Signing in again will not change that access/);
    expect(screen.queryByRole('link', { name: 'Sign in again' })).toBeNull();
    expect(
      within(screen.getByRole('dialog', { name: 'Service authorization' })).getByRole('status'),
    ).toHaveTextContent('The account grant status could not be verified.');
  });
  it('keeps the original tab when its query cannot be restored through ordinary sign-in', async () => {
    window.history.replaceState(null, '', '/event-gateway?unsupported=state');
    try {
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => new Response(null, { status: 401 })),
      );
      const actor = userEvent.setup();
      render(<ProfileMenu user={{ ...user, serviceAuthorization: true }} onSignOut={vi.fn()} />);
      await actor.click(screen.getByRole('button', { name: /account menu/i }));
      await actor.click(screen.getByRole('menuitem', { name: 'Service authorization' }));
      const link = await screen.findByRole('link', { name: 'Sign in again' });
      expect(link).toHaveAttribute('href', '/auth/login');
      expect(link).toHaveAttribute('target', '_blank');
    } finally {
      window.history.replaceState(null, '', '/');
    }
  });
  it.each([401, 403, 503])(
    'offers ordinary sign-in only for a confirmed 401 (%i)',
    async (status) => {
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => new Response(null, { status })),
      );
      const actor = userEvent.setup();
      render(<ProfileMenu user={{ ...user, serviceAuthorization: true }} onSignOut={vi.fn()} />);
      await actor.click(screen.getByRole('button', { name: /account menu/i }));
      await actor.click(screen.getByRole('menuitem', { name: 'Service authorization' }));
      if (status === 401) {
        const link = await screen.findByRole('link', { name: 'Sign in again' });
        expect(link).toHaveAttribute('href', '/auth/login?return_to=%2F');
        expect(link).toHaveAttribute('target', '_blank');
      } else {
        await screen.findByText('The account grant status could not be verified.');
        expect(screen.queryByRole('link', { name: 'Sign in again' })).toBeNull();
      }
    },
  );
  it('keeps Docs, appearance and sign out without service or developer actions', async () => {
    const onSignOut = vi.fn();
    const onNavigate = vi.fn();
    const actor = userEvent.setup();
    render(<ProfileMenu user={user} onSignOut={onSignOut} onNavigate={onNavigate} />);
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    for (const name of ['Event Gateway consumers', 'Developer', 'Service authorization']) {
      expect(screen.queryByRole('menuitem', { name })).not.toBeInTheDocument();
    }
    expect(screen.queryByRole('menuitem', { name: 'Marketplace' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Operations' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'CMA Trigger' })).not.toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Appearance' })).toBeInTheDocument();

    await actor.click(screen.getByRole('menuitem', { name: 'Docs' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'docs' });
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Sign out' }));
    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it.each([
    {
      status: 'partial',
      text: 'The account grant is limited to selected services. Other services may need authorization.',
    },
    {
      status: 'expired',
      text: 'The account grant has expired. Authorize required services from your draft to continue.',
    },
    {
      status: 'not_granted',
      text: 'No current account grant was found. It may not have been granted or may have been revoked.',
    },
    { status: 'unknown', text: 'The account grant status could not be verified.' },
    { status: 'revoked', text: 'NyxID confirms the account grant was revoked.' },
  ])(
    'shows $status for the account grant when the feature is enabled',
    async ({ status, text }) => {
      const actor = userEvent.setup();
      vi.stubGlobal(
        'fetch',
        vi.fn(async () => Response.json({ owner: user.id, status })),
      );
      render(<ProfileMenu user={{ ...user, serviceAuthorization: true }} onSignOut={vi.fn()} />);
      await actor.click(screen.getByRole('button', { name: /account menu/i }));
      await actor.click(screen.getByRole('menuitem', { name: 'Service authorization' }));
      expect(await screen.findByText(text)).toBeInTheDocument();
      expect(screen.queryByRole('link', { name: 'Manage authorized apps in NyxID' })).toBeNull();
    },
  );

  it('shows Operations only with an operator capability', async () => {
    const actor = userEvent.setup();
    render(
      <ProfileMenu
        user={{ ...user, capabilities: ['read_activity_log'] }}
        onSignOut={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    expect(screen.getByRole('menuitem', { name: 'Operations' })).toBeInTheDocument();
  });
});

it('derives stable initials from the display name', () => {
  expect(initialsFor(user)).toBe('AL');
});

it('provides docs and explicit text sizes without the relocated Agent/trigger actions', async () => {
  const actor = userEvent.setup();
  const navigate = vi.fn();
  render(
    <ProfileMenu
      user={{
        ...user,
        capabilities: ['use_own_sandboxes', 'publish_triggers', 'read_activity_log'],
      }}
      onNavigate={navigate}
      onSignOut={vi.fn()}
    />,
  );
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  expect(screen.queryByRole('menuitem', { name: 'Agents' })).not.toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: 'CMA Trigger' })).not.toBeInTheDocument();
  const menu = screen.getByRole('menu');
  const docs = within(menu).getByRole('menuitem', { name: 'Docs' });
  const ops = within(menu).getByRole('menuitem', { name: 'Operations' });
  expect(docs.compareDocumentPosition(ops) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const sizes = screen.getByRole('group', { name: 'System text size' });
  await actor.click(within(sizes).getByRole('menuitemradio', { name: 'Large' }));
  expect(within(sizes).getByRole('menuitemradio', { name: 'Large' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  expect(document.documentElement.dataset.systemTextSize).toBe('large');
  expect(localStorage.getItem('cma:system-text-size')).toBe('large');
  await actor.click(docs);
  expect(navigate).toHaveBeenCalledWith({ kind: 'docs' });
});

it('keeps text preferences in sync between retained portal account menus', async () => {
  const actor = userEvent.setup();
  const one = render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  const two = render(
    <ProfileMenu user={{ ...user, displayName: 'Second surface' }} onSignOut={vi.fn()} />,
  );
  await actor.click(within(one.container).getByRole('button'));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Small' }));
  await actor.click(within(two.container).getByRole('button'));
  expect(screen.getByRole('menuitemradio', { name: 'Small' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await actor.click(screen.getByRole('menuitemradio', { name: 'Medium' }));
});

it('copies all three serving build identities even when the Gateway is unready', async () => {
  const fetch = mockBuilds();
  const writeText = vi.fn().mockResolvedValue(undefined);
  // After setup, which installs its own clipboard on the navigator.
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} onNavigate={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(await screen.findByRole('menuitem', { name: buildLabel }));
  expect(writeText).toHaveBeenCalledWith(copiedBuilds);
  for (const path of [amPath, egPath]) {
    expect(fetch).toHaveBeenCalledWith(
      new URL(path, window.location.origin),
      expect.objectContaining({ credentials: 'omit', cache: 'no-store' }),
    );
  }
  expect(await screen.findByRole('menuitem', { name: 'Build version copied' })).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Build version copied');
  expect(screen.getByRole('menu')).not.toContainElement(screen.getByRole('status'));
});

it('keeps the three revisions readable and selected when the clipboard refuses them', async () => {
  mockBuilds();
  const writeText = vi.fn().mockRejectedValue(new Error('denied'));
  // After setup, which installs its own clipboard on the navigator.
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} onNavigate={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(await screen.findByRole('menuitem', { name: buildLabel }));
  expect(writeText).toHaveBeenCalledOnce();
  expect(screen.getByRole('status')).toHaveTextContent('Could not copy');
  expect(screen.getByText(copiedBuilds)).toBeInTheDocument();
  expect(window.getSelection()?.toString()).toBe(copiedBuilds);
  writeText.mockResolvedValue(undefined);
  await actor.click(screen.getByRole('menuitem', { name: /Copy build version/ }));
  expect(screen.getByRole('status')).toHaveTextContent('Build version copied');
});

it('provides a selectable identity when the Clipboard API is absent', async () => {
  mockBuilds();
  const actor = userEvent.setup();
  Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(await screen.findByRole('menuitem', { name: buildLabel }));
  expect(screen.getByRole('status')).toHaveTextContent('Could not copy');
  expect(screen.getByText(copiedBuilds)).toBeInTheDocument();
  expect(screen.getByRole('menuitem', { name: /Copy build version/ })).toHaveAttribute(
    'aria-disabled',
    'false',
  );
});

it.each([
  ['missing revision', { version: amBuild.version }],
  ['hostname-shaped version', { ...amBuild, version: 'node.example.test' }],
  ['mismatched git version', { ...amBuild, version: 'git-cccccccccccc' }],
])('marks Agent Manager as unavailable with a %s and still copies Gateway', async (_, stamp) => {
  vi.stubGlobal('fetch', async (url: URL) =>
    url.pathname === amPath ? Response.json(stamp) : Response.json({ ready: true, build: egBuild }),
  );
  const actor = userEvent.setup();
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  await actor.click(
    await screen.findByRole('menuitem', {
      name: 'Copy build version · Web 0.20.22 · AM unavailable · EG 0.11.16',
    }),
  );
  expect(writeText).toHaveBeenCalledWith(
    `Web 0.20.22 (${'a'.repeat(40)}) · AM unavailable · EG 0.11.16 (${'c'.repeat(40)})`,
  );
});

it('accepts a matching Agent Manager git version when the Gateway proxy returns its own 503', async () => {
  vi.stubGlobal('fetch', async (url: URL) =>
    url.pathname === amPath
      ? Response.json({ ...amBuild, version: 'git-bbbbbbbbbbbb' })
      : Response.json({ error: 'unavailable' }, { status: 503 }),
  );
  const actor = userEvent.setup();
  render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  await actor.click(screen.getByRole('button', { name: /account menu/i }));
  expect(
    await screen.findByRole('menuitem', {
      name: 'Copy build version · Web 0.20.22 · AM git-bbbbbbbbbbbb · EG unavailable',
    }),
  ).toHaveAttribute('aria-disabled', 'false');
});

it('bounds a slow Gateway lookup and preserves the healthy Agent Manager result', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('fetch', async (url: URL, options: RequestInit) => {
    if (url.pathname === amPath) return Response.json(amBuild);
    return new Promise<Response>((_resolve, reject) => {
      const signal = options.signal!;
      signal.addEventListener('abort', () => reject(signal.reason), { once: true });
    });
  });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: /account menu/i }));
  expect(screen.getByRole('menuitem', { name: /Copy build version.*checking/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  await act(async () => vi.advanceTimersByTimeAsync(BUILD_LOOKUP_TIMEOUT_MS));
  expect(
    screen.getByRole('menuitem', {
      name: 'Copy build version · Web 0.20.22 · AM 0.22.34 · EG unavailable',
    }),
  ).toHaveAttribute('aria-disabled', 'false');
});

it('replaces Settings with Usage and focuses the retained trigger before navigation', async () => {
  const actor = userEvent.setup();
  let focused: Element | null = null;
  const onNavigate = vi.fn(() => {
    focused = document.activeElement;
  });
  render(<ProfileMenu user={user} onSignOut={vi.fn()} onNavigate={onNavigate} />);
  const trigger = screen.getByRole('button', { name: /Account menu for/ });
  await actor.click(trigger);
  expect(screen.queryByRole('menuitem', { name: 'Settings' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('menuitem', { name: 'Usage' }));
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'usage' });
  expect(focused).toBe(trigger);
});
it('offers one explicit Appearance group and synchronizes retained menus and storage', async () => {
  const actor = userEvent.setup();
  const one = render(<ProfileMenu user={user} onSignOut={vi.fn()} />);
  const two = render(
    <ProfileMenu user={{ ...user, displayName: 'Second surface' }} onSignOut={vi.fn()} />,
  );
  await actor.click(within(one.container).getByRole('button'));
  const appearance = screen.getByRole('group', { name: 'Appearance' });
  for (const value of ['System', 'Light', 'Dark']) {
    await actor.click(within(appearance).getByRole('menuitemradio', { name: value }));
    expect(localStorage.getItem('cma:appearance')).toBe(value.toLowerCase());
    expect(within(appearance).getByRole('menuitemradio', { name: value })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  }
  await actor.click(within(two.container).getByRole('button'));
  expect(screen.getByRole('menuitemradio', { name: 'Dark' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  localStorage.setItem('cma:appearance', 'light');
  window.dispatchEvent(new StorageEvent('storage', { key: 'cma:appearance' }));
  await screen.findByRole('menuitemradio', { name: 'Light', checked: true });
});
