import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SignInCard } from './SignInCard';

/**
 * The sign-in card — §18, for a session spending its owner's own account.
 *
 * The card's whole job is to be actionable from another device, so what it
 * must never do is show something that cannot be acted on.
 */

const ASKED = {
  url: 'https://example.invalid/activate',
  code: 'ABCD-EFGH',
  message: 'Sign in and enter this code',
};

describe('the sign-in card', () => {
  it('shows where to go and what to type', () => {
    render(<SignInCard signIn={ASKED} />);
    expect(screen.getByText('ABCD-EFGH')).toBeInTheDocument();
    const link = screen.getByRole('link');
    expect(link).toHaveAttribute('href', ASKED.url);
    // The page is untrusted by construction: this address came from a kernel.
    expect(link).toHaveAttribute('rel', expect.stringContaining('noopener'));
  });

  it('carries the kernel’s own wording when it gave any', () => {
    // §18.2 and §18.10 in identical words: the kernel knows what it is asking
    // for and this client does not.
    render(<SignInCard signIn={ASKED} />);
    expect(screen.getByText(ASKED.message)).toBeInTheDocument();
  });

  it('says something useful when the kernel said nothing', () => {
    render(<SignInCard signIn={{ url: ASKED.url }} />);
    expect(screen.getByText(/runs on your own account/i)).toBeInTheDocument();
  });

  it('draws no code block when no code was given', () => {
    // An empty box where a code should be reads as a code that failed to
    // arrive, which is worse than a card that never claimed to have one.
    const { container } = render(<SignInCard signIn={{ url: ASKED.url }} />);
    expect(container.querySelector('.signin__code')).toBeNull();
  });

  it('names the precondition a subscription does not cover', () => {
    // Device sign-in is off by default on a consumer account and controlled by
    // an administrator on a managed one. §9: said up front rather than
    // discovered halfway through.
    render(<SignInCard signIn={ASKED} />);
    expect(screen.getByText(/security settings/i)).toBeInTheDocument();
  });

  it('is not a modal', () => {
    // A session can wait here while its owner reads something else; a modal
    // would make every other session unreachable until they finished.
    const { container } = render(<SignInCard signIn={ASKED} />);
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(container.querySelector('[aria-modal]')).toBeNull();
  });

  it('renders no credential, because it is given none', () => {
    // §18.7. What travels here is an address and a short-lived code, both
    // worthless without the person's own account.
    const { container } = render(<SignInCard signIn={ASKED} />);
    for (const secret of ['Bearer', 'sk-', 'auth.json', 'access_token']) {
      expect(container.textContent ?? '').not.toContain(secret);
    }
  });
});
