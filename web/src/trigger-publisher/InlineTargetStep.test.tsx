import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import {
  listAgentImages,
  resolveAgentImage,
  type ResolvedAgentImage,
} from '../create/imageCatalog';
import { TriggerWizard } from './TriggerWizard';
import { draft, publisherFixture } from './fixtures';
import { blankContent } from './model';

vi.mock('../create/imageCatalog', () => ({
  listAgentImages: vi.fn(),
  resolveAgentImage: vi.fn(),
  imageVisibilityLabel: () => 'Private',
}));
beforeEach(() => {
  vi.mocked(listAgentImages).mockResolvedValue([{ id: 'img_custom', name: 'Custom runtime' }]);
});
function show() {
  const content = blankContent();
  const document = { ...draft, draft: { ...content, presentation: draft.draft.presentation } };
  const fixture = publisherFixture(document);
  render(
    <TriggerWizard
      client={fixture.client}
      document={document}
      onChange={vi.fn()}
      onPublished={vi.fn()}
      onLock={vi.fn()}
      onSaved={vi.fn()}
    />,
  );
  return fixture;
}

it('waits for the selected image and preserves edits made while its immutable version resolves', async () => {
  let finish!: (image: ResolvedAgentImage) => void;
  vi.mocked(resolveAgentImage).mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const fixture = show();
  const user = userEvent.setup();
  await screen.findByRole('option', { name: 'Custom runtime (Private)' });
  const image = screen.getByRole('combobox', { name: 'Image' });
  await waitFor(() => expect(image).toBeEnabled());
  await user.selectOptions(image, 'img_custom');
  expect(image).toHaveValue('img_custom');
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Save draft' })).toBeDisabled();
  const model = screen.getByRole('textbox', { name: 'Model' });
  await waitFor(() => expect(model).toBeEnabled());
  await user.type(model, 'my-model');
  await user.type(screen.getByRole('textbox', { name: 'System prompt' }), 'Keep my edits');
  const resolved = {
    id: 'img_custom',
    version: 'build_one',
    reference: `registry.example/runtime@sha256:${'a'.repeat(64)}`,
  };
  await act(async () => {
    finish(resolved);
  });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  expect(model).toHaveValue('my-model');
  expect(screen.getByRole('textbox', { name: 'System prompt' })).toHaveValue('Keep my edits');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(fixture.triggers.save).toHaveBeenCalledOnce());
  expect(fixture.triggers.save.mock.calls[0]?.[1].content.target.inline).toMatchObject({
    image: resolved,
    agent: { model: 'my-model', system_prompt: 'Keep my edits' },
  });
  for (let step = 0; step < 4; step += 1) {
    const next = screen.getByRole('button', { name: 'Next' });
    await waitFor(() => expect(next).toBeEnabled());
    await user.click(next);
  }
  expect(screen.getByRole('heading', { name: 'Publish' })).toBeInTheDocument();
  expect(screen.getByText('img_custom')).toBeInTheDocument();
  expect(screen.getByText('build_one')).toBeInTheDocument();
  expect(screen.getByText('my-model')).toBeInTheDocument();
  expect(screen.getByText('Keep my edits')).toBeInTheDocument();
  expect(screen.queryByText(/Profile unavailable/)).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: /profile/i })).not.toBeInTheDocument();
});

it('requires an explicit new image choice after resolution fails', async () => {
  vi.mocked(resolveAgentImage).mockRejectedValue(new Error('Version unavailable'));
  const fixture = show();
  const user = userEvent.setup();
  await screen.findByRole('option', { name: 'Custom runtime (Private)' });
  const image = screen.getByRole('combobox', { name: 'Image' });
  await waitFor(() => expect(image).toBeEnabled());
  await user.selectOptions(image, 'img_custom');
  await screen.findByText('The selected image has no successful immutable version.');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'What it opens' })).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('no immutable successful version');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(fixture.triggers.save).not.toHaveBeenCalled();
  await user.selectOptions(image, '');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('heading', { name: 'When someone clicks again' })).toBeInTheDocument();
});
