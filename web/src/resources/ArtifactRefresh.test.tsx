import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it } from 'vitest';
import { ArtifactButton } from './ArtifactButton';

it('preserves the file input and selected approval across focus rechecks without permitting publication', async () => {
  let holdAccess = false;
  const pendingAccess: ((value: Response) => void)[] = [];
  let publications = 0;
  const access = () =>
    Response.json({
      capabilities: ['view_artifacts', 'manage_sharing'],
      revalidate_at: null,
      policy_revision: 1,
    });
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input, init) => {
      if (init?.method === 'POST') publications += 1;
      if (String(input).includes('resource-access'))
        return holdAccess
          ? new Promise<Response>((resolve) => pendingAccess.push(resolve))
          : access();
      return Response.json({ items: [] });
    },
  });
  render(<ArtifactButton client={client} id="agt_1" />);
  fireEvent.click(await screen.findByRole('button', { name: 'Artifacts' }));
  const originalInput = await screen.findByLabelText<HTMLInputElement>('Artifact file');
  const file = new File(['retained draft'], 'Draft.txt', { type: 'text/plain' });

  // Native file pickers can return focus before delivering the input change.
  holdAccess = true;
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(pendingAccess.length).toBeGreaterThan(0));
  expect(screen.getByLabelText('Artifact file')).toBe(originalInput);
  fireEvent.change(originalInput, { target: { files: [file] } });
  holdAccess = false;
  await act(async () => pendingAccess.splice(0).forEach((finish) => finish(access())));
  await waitFor(() => expect(screen.getByRole('checkbox')).toBeEnabled());
  expect(originalInput.files?.[0]).toBe(file);
  fireEvent.click(screen.getByRole('checkbox'));
  expect(screen.getByRole('button', { name: 'Publish artifact' })).toBeEnabled();

  holdAccess = true;
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(pendingAccess.length).toBeGreaterThan(0));
  expect(screen.getByLabelText('Artifact file')).toBe(originalInput);
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('button', { name: 'Publish artifact' })).toBeDisabled();
  expect(publications).toBe(0);
  holdAccess = false;
  await act(async () => pendingAccess.splice(0).forEach((finish) => finish(access())));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Publish artifact' })).toBeEnabled(),
  );
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(originalInput.files?.[0]).toBe(file);
  expect(publications).toBe(0);
});
