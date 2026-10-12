import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { LandingCharacter } from './LandingCharacter';
import { SignInLanding } from './SignInLanding';

afterEach(() => vi.useRealTimers());

describe('the signed-out invitation', () => {
  it('reveals the cloud after five seconds, not glasses lettering', () => {
    vi.useFakeTimers();
    render(<SignInLanding onSignIn={vi.fn()} />);
    const character = screen.getByRole('button', { name: 'Get Started' });
    expect(character).not.toHaveAttribute('data-invitation');
    act(() => vi.advanceTimersByTime(4_999));
    expect(character).not.toHaveAttribute('data-invitation');
    act(() => vi.advanceTimersByTime(1));
    expect(character).toHaveAttribute('data-invitation');
    expect(screen.getByText('Click to start...')).toBeInTheDocument();
    expect(screen.queryByText('Get')).not.toBeInTheDocument();
    expect(screen.queryByText('Started')).not.toBeInTheDocument();
  });

  it.each(['hover', 'focus'])('reveals immediately on %s', (interaction) => {
    render(<SignInLanding onSignIn={vi.fn()} />);
    const character = screen.getByRole('button', { name: 'Get Started' });
    if (interaction === 'hover') fireEvent.pointerEnter(character);
    else fireEvent.focus(character);
    expect(character).toHaveAttribute('data-invitation');
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

  it('starts fallback sign-in only once after the fade', async () => {
    vi.useFakeTimers();
    const signIn = vi.fn().mockResolvedValue(undefined);
    render(<SignInLanding onSignIn={signIn} />);
    const character = screen.getByRole('button', { name: 'Get Started' });
    fireEvent.click(character);
    fireEvent.click(character);
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(signIn).toHaveBeenCalledTimes(1);
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
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(signIn).toHaveBeenCalledTimes(2);
  });
});
