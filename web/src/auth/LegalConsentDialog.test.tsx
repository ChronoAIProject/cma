import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ApiError, type LegalConsent } from '@cma/client';
import { LegalConsentDialog } from './LegalConsentDialog';

const terms = {
  version: '2026-11-30',
  termsUrl: 'https://legal.example.test/terms',
  privacyUrl: 'https://legal.example.test/privacy',
} as const;

function consent(acceptedAt: string | null, version: string = terms.version): LegalConsent {
  return {
    version,
    terms_url: terms.termsUrl,
    privacy_url: terms.privacyUrl,
    accepted_at: acceptedAt,
  };
}

function refusal(status: number, code: string) {
  return new ApiError(status, code, code);
}

function client(
  status: () => Promise<LegalConsent>,
  accept: () => Promise<LegalConsent> = async () => consent('2026-11-30T01:00:00Z'),
) {
  return { status: vi.fn(status), accept: vi.fn(accept) };
}

describe('legal consent after sign-in', () => {
  it('shows nothing once the current version has been accepted', async () => {
    const api = client(async () => consent('2026-11-30T00:00:00Z'));
    render(<LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />);
    await waitFor(() => expect(api.status).toHaveBeenCalledOnce());
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('cannot be dismissed and records the exact version on acceptance', async () => {
    const api = client(async () => consent(null));
    const actor = userEvent.setup();
    render(<LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />);
    const dialog = await screen.findByRole('dialog', { name: 'Review the terms' });
    expect(dialog).toHaveTextContent('version 2026-11-30');
    expect(screen.getByRole('link', { name: 'Terms of Service' })).toHaveAttribute(
      'href',
      terms.termsUrl,
    );
    expect(screen.getByRole('link', { name: 'Privacy Notice' })).toHaveAttribute(
      'href',
      terms.privacyUrl,
    );
    expect(screen.getByRole('button', { name: 'Close' })).toBeDisabled();
    await actor.keyboard('{Escape}');
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    await actor.click(screen.getByRole('button', { name: 'Accept and continue' }));
    expect(api.accept).toHaveBeenCalledWith({ version: '2026-11-30' });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('asks again when only an earlier version was accepted', async () => {
    const api = client(async () => consent('2026-11-01T00:00:00Z', '2026-11-01'));
    render(<LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />);
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
  });

  it('offers sign-out instead of accepting', async () => {
    const onSignOut = vi.fn();
    const actor = userEvent.setup();
    render(
      <LegalConsentDialog
        terms={terms}
        client={client(async () => consent(null))}
        onSignOut={onSignOut}
      />,
    );
    await actor.click(await screen.findByRole('button', { name: 'Sign out' }));
    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it('stays out of the way when no consent can be recorded for this session', async () => {
    for (const error of [
      refusal(404, 'legal_terms_not_configured'),
      refusal(403, 'legal_consent_requires_owner_session'),
      refusal(401, 'not_authenticated'),
      refusal(403, 'account_deletion_in_progress'),
    ]) {
      const api = client(async () => Promise.reject(error));
      const { unmount } = render(
        <LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />,
      );
      await waitFor(() => expect(api.status).toHaveBeenCalledOnce());
      expect(screen.queryByRole('dialog')).toBeNull();
      unmount();
    }
  });

  it('keeps blocking with a retry when the record cannot be checked', async () => {
    let calls = 0;
    const api = client(async () => {
      calls += 1;
      if (calls === 1) throw refusal(503, 'store_failed');
      return consent('2026-11-30T00:00:00Z');
    });
    const actor = userEvent.setup();
    render(<LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'We could not check whether you have accepted these terms.',
    );
    expect(screen.queryByRole('button', { name: 'Accept and continue' })).toBeNull();
    await actor.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('explains a failed or outdated acceptance and lets the owner retry', async () => {
    const api = client(
      async () => consent(null),
      vi
        .fn<() => Promise<LegalConsent>>()
        .mockRejectedValueOnce(refusal(503, 'store_failed'))
        .mockRejectedValueOnce(refusal(409, 'legal_consent_sign_in_required'))
        .mockRejectedValueOnce(refusal(409, 'legal_terms_version_mismatch')),
    );
    const actor = userEvent.setup();
    render(<LegalConsentDialog terms={terms} client={api} onSignOut={vi.fn()} />);
    await actor.click(await screen.findByRole('button', { name: 'Accept and continue' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Your acceptance could not be recorded. Try again.',
    );
    await actor.click(screen.getByRole('button', { name: 'Accept and continue' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Your sign-in could not be confirmed. Sign out and sign in again to accept the terms.',
    );
    await actor.click(screen.getByRole('button', { name: 'Accept and continue' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'These terms have been updated. Reload the page to review the current version.',
    );
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });
});
