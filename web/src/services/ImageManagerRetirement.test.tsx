import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { SignInLanding } from '../landing/SignInLanding';
import { ImageManagerRetirement } from './ImageManagerRetirement';
import { ServiceMenu } from './ServiceMenu';

afterEach(() => vi.unstubAllGlobals());

it.each([
  [200, 'true', false],
  [200, 'false', true],
  [404, '{"code":"not_found"}', true],
] as const)(
  'applies the API capability response %s %s to navigation',
  async (status, body, visible) => {
    const fetcher = vi.fn().mockResolvedValue(new Response(body, { status }));
    vi.stubGlobal('fetch', fetcher);
    render(
      <ImageManagerRetirement>
        <SignInLanding onSignIn={vi.fn()} />
        <ServiceMenu route={{ kind: 'agent_manager' }} onNavigate={vi.fn()} />
      </ImageManagerRetirement>,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Service: Agent Manager' }));
    await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
    expect(fetcher).toHaveBeenCalledWith('/api/v1/ui/image-manager-retirement', expect.any(Object));
    if (visible) {
      expect(await screen.findByRole('heading', { name: /Image Manager/ })).toBeInTheDocument();
      expect(await screen.findByRole('menuitemradio', { name: /Image Manager/ })).toHaveAttribute(
        'aria-disabled',
        'true',
      );
    } else {
      await waitFor(() =>
        expect(screen.queryByRole('heading', { name: /Image Manager/ })).not.toBeInTheDocument(),
      );
      expect(
        screen.queryByRole('menuitemradio', { name: /Image Manager/ }),
      ).not.toBeInTheDocument();
      expect(screen.getByRole('menuitemradio', { name: 'Event Gateway' })).toBeInTheDocument();
    }
  },
);
