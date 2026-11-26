import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { LandingCharacter } from './LandingCharacter';
import { SignInLanding } from './SignInLanding';

afterEach(() => vi.useRealTimers());

describe('the signed-out landing', () => {
  it('says what CMA is and offers one visible primary action', () => {
    render(<SignInLanding onSignIn={vi.fn()} />);
    expect(
      screen.getByRole('heading', { level: 1, name: 'ChronoAI Managed Agent' }),
    ).toBeInTheDocument();
    expect(screen.getByText(/A little direction.*A world of possibility/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Get Started' })).toBeEnabled();
    expect(screen.getByText('Sign in with NyxID. You’ll come straight back.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'See how it works' })).toHaveAttribute(
      'href',
      '#sign-in-landing-how',
    );
  });

  it('links the published terms beside the way in, and only when they exist', () => {
    const { rerender } = render(<SignInLanding onSignIn={vi.fn()} />);
    expect(screen.queryByRole('link', { name: 'Terms of Service' })).toBeNull();
    rerender(
      <SignInLanding
        onSignIn={vi.fn()}
        legalTerms={{
          version: '2026-11-30',
          termsUrl: 'https://legal.example.test/terms',
          privacyUrl: 'https://legal.example.test/privacy',
        }}
      />,
    );
    expect(screen.getByText(/By continuing, you agree to the/)).toBeInTheDocument();
    for (const [name, href] of [
      ['Terms of Service', 'https://legal.example.test/terms'],
      ['Privacy Notice', 'https://legal.example.test/privacy'],
    ]) {
      const link = screen.getByRole('link', { name });
      expect(link).toHaveAttribute('href', href);
      expect(link).toHaveAttribute('target', '_blank');
      expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    }
  });

  it('never names the agent provider in its branding', () => {
    const { container } = render(<SignInLanding onSignIn={vi.fn()} />);
    expect(container.textContent).not.toMatch(/codex/i);
  });

  it('introduces the services, marking Image Manager as not yet available', () => {
    render(<SignInLanding onSignIn={vi.fn()} />);
    const services = screen.getByRole('region', {
      name: 'Your work, connected.',
    });
    for (const name of ['Agent Manager', 'Event Gateway']) {
      expect(screen.getByRole('button', { name: `Open ${name}` })).toBeEnabled();
    }
    expect(services).toHaveTextContent('Image Manager Coming soon');
    expect(screen.queryByRole('button', { name: 'Open Image Manager' })).not.toBeInTheDocument();
  });

  it('explains how it works in three ordered steps', () => {
    render(<SignInLanding onSignIn={vi.fn()} />);
    const how = screen.getByRole('region', { name: 'From a first thought to work in motion' });
    expect(how.querySelectorAll('ol > li')).toHaveLength(3);
  });

  it('keeps the character as a companion rather than the way in', () => {
    vi.useFakeTimers();
    render(<SignInLanding onSignIn={vi.fn()} />);
    act(() => vi.advanceTimersByTime(5_000));
    expect(screen.queryByText('Click to start...')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Spin CMA character' })).not.toHaveAttribute(
      'data-activation',
    );
  });

  it('does not add invitations or activation to signed-in characters', () => {
    vi.useFakeTimers();
    render(<LandingCharacter autoRotate />);
    const character = screen.getByRole('button', { name: 'Spin CMA character' });
    fireEvent.pointerEnter(character);
    fireEvent.focus(character);
    act(() => vi.advanceTimersByTime(5_000));
    expect(character).not.toHaveAttribute('data-invitation');
    expect(character).not.toHaveAttribute('data-activation');
    expect(screen.queryByText('Click to start...')).not.toBeInTheDocument();
  });

  it('starts sign-in only once after the fade and announces it', async () => {
    vi.useFakeTimers();
    const signIn = vi.fn().mockResolvedValue(undefined);
    render(<SignInLanding onSignIn={signIn} />);
    const start = screen.getByRole('button', { name: 'Get Started' });
    fireEvent.click(start);
    fireEvent.click(start);
    expect(screen.getByRole('status')).toHaveTextContent('Opening NyxID sign-in…');
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(signIn).toHaveBeenCalledTimes(1);
    expect(signIn).toHaveBeenCalledWith(undefined);
  });

  it('returns a service card to its own service after sign-in', async () => {
    vi.useFakeTimers();
    const signIn = vi.fn().mockResolvedValue(undefined);
    render(<SignInLanding onSignIn={signIn} />);
    fireEvent.click(screen.getByRole('button', { name: 'Open Event Gateway' }));
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(signIn).toHaveBeenCalledWith('/event-gateway');
  });

  it('restores a usable retry when sign-in fails', async () => {
    vi.useFakeTimers();
    const signIn = vi
      .fn()
      .mockRejectedValueOnce(new Error('Unavailable'))
      .mockResolvedValue(undefined);
    render(<SignInLanding onSignIn={signIn} />);
    fireEvent.click(screen.getByRole('button', { name: 'Get Started' }));
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(screen.getByRole('alert')).toHaveTextContent('Sign-in could not be started.');
    expect(screen.getByRole('button', { name: 'Get Started' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(signIn).toHaveBeenCalledTimes(2);
  });
});
