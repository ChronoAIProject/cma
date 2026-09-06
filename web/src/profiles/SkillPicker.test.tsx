import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { SkillPicker } from './pickers/SkillPicker';

it('does not add a skill without a literal published release', async () => {
  const onChange = vi.fn();
  render(
    <SkillPicker
      catalog={{
        searchSkills: async () => ({
          items: [
            { id: 'empty', name: 'Unreleased', versions: [] },
            { id: 'invalid', name: 'Nonliteral', versions: ['latest', '1.2.3'] },
          ],
        }),
      }}
      value={[]}
      onChange={onChange}
      disabled={false}
    />,
  );
  expect(await screen.findByRole('button', { name: 'Add Unreleased' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Add Nonliteral' })).toBeDisabled();
  expect(onChange).not.toHaveBeenCalled();
});

it('retains the selected pin when the catalogue fails and exposes its help to assistive technology', async () => {
  const onChange = vi.fn();
  render(
    <SkillPicker
      catalog={{
        searchSkills: async () => {
          throw new Error('Unavailable');
        },
      }}
      value={[{ source: 'ornn', id: 'research', version: '1.1' }]}
      onChange={onChange}
      disabled={false}
    />,
  );
  await screen.findByText('Skills could not be loaded.');
  const version = screen.getByRole('button', { name: 'Version for research' });
  expect(version).toHaveTextContent('1.1');
  expect(version).toBeDisabled();
  expect(version).toHaveAccessibleDescription(
    'Release versions are unavailable. The selected pin is unchanged.',
  );
  expect(onChange).not.toHaveBeenCalled();
});

it('offers an existing unavailable pin as unavailable without silently replacing it', async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  render(
    <SkillPicker
      catalog={{
        searchSkills: async () => ({
          items: [{ id: 'research', name: 'Research', versions: ['1.2'] }],
        }),
      }}
      value={[{ source: 'ornn', id: 'research', version: '1.1' }]}
      onChange={onChange}
      disabled={false}
    />,
  );
  await screen.findByRole('button', { name: 'Add Research' });
  await user.click(screen.getByRole('button', { name: 'Version for research' }));
  expect(screen.getByRole('menuitemradio', { name: /1.1/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  expect(onChange).not.toHaveBeenCalled();
  await user.click(screen.getByRole('menuitemradio', { name: '1.2' }));
  expect(onChange).toHaveBeenCalledWith([{ source: 'ornn', id: 'research', version: '1.2' }]);
});
