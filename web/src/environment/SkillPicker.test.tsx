import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SkillPicker } from './SkillPicker';
import type { CatalogSkill } from './types';

function skill(id: string, description = ''): CatalogSkill {
  return { id, name: id, description };
}

const FEW: readonly CatalogSkill[] = [
  skill('nyxid', 'Broker credentials.'),
  skill('ripgrep-helper', 'Search a repository quickly.'),
];

/** More than the threshold at which a search field appears. */
const MANY: readonly CatalogSkill[] = Array.from({ length: 12 }, (_, i) =>
  skill(`skill-${i}`, i === 3 ? 'the needle' : 'a haystack entry'),
);

describe('SkillPicker', () => {
  it('selects and deselects a skill', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SkillPicker skills={FEW} selected={[]} mandatory={[]} onChange={onChange} />);

    await user.click(screen.getByRole('checkbox', { name: /ripgrep-helper/i }));
    expect(onChange).toHaveBeenCalledWith(['ripgrep-helper']);

    onChange.mockClear();
    render(
      <SkillPicker skills={FEW} selected={['ripgrep-helper']} mandatory={[]} onChange={onChange} />,
    );
    await user.click(
      screen.getAllByRole('checkbox', { name: /ripgrep-helper/i })[1] as HTMLElement,
    );
    expect(onChange).toHaveBeenCalledWith([]);
  });

  it('will not deselect a mandatory skill even if the control is reached', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SkillPicker skills={FEW} selected={[]} mandatory={['nyxid']} onChange={onChange} />);

    const required = screen.getByRole('checkbox', { name: /nyxid/i });
    expect(required).toBeChecked();
    expect(required).toBeDisabled();
    await user.click(required);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('offers a search only once the list is long enough to need one', async () => {
    const user = userEvent.setup();
    const { unmount } = render(
      <SkillPicker skills={FEW} selected={[]} mandatory={[]} onChange={vi.fn()} />,
    );
    expect(screen.queryByLabelText(/search skills/i)).toBeNull();
    unmount();

    render(<SkillPicker skills={MANY} selected={[]} mandatory={[]} onChange={vi.fn()} />);
    await user.type(screen.getByLabelText(/search skills/i), 'needle');

    expect(screen.getAllByRole('checkbox')).toHaveLength(1);
    expect(screen.getByRole('checkbox', { name: /skill-3/i })).toBeInTheDocument();
  });

  it('says when a search matches nothing, distinctly from having nothing', async () => {
    const user = userEvent.setup();
    render(<SkillPicker skills={MANY} selected={[]} mandatory={[]} onChange={vi.fn()} />);

    await user.type(screen.getByLabelText(/search skills/i), 'no-such-skill');

    expect(screen.getByText(/nothing matches that search/i)).toBeInTheDocument();
  });

  it('says when the registry has nothing for this user', () => {
    render(<SkillPicker skills={[]} selected={[]} mandatory={[]} onChange={vi.fn()} />);
    expect(screen.getByText(/no skills are available/i)).toBeInTheDocument();
  });

  it('explains a registry failure and says the profile still saves', () => {
    render(
      <SkillPicker
        skills={[]}
        selected={[]}
        mandatory={[]}
        onChange={vi.fn()}
        error="The registry is unreachable."
      />,
    );

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('The registry is unreachable.');
    // Web_Style.md section 7: say what to do about it.
    expect(alert).toHaveTextContent(/still saves/i);
  });

  it('shows why a shared skill is visible, but not for mandatory ones', () => {
    render(
      <SkillPicker
        skills={[
          { id: 'shared', name: 'shared', description: '', access: 'shared-via-org' },
          { id: 'nyxid', name: 'nyxid', description: '', access: 'public' },
        ]}
        selected={[]}
        mandatory={['nyxid']}
        onChange={vi.fn()}
      />,
    );

    expect(screen.getByText('shared-via-org')).toBeInTheDocument();
    // A required skill's badge is "required"; how it became visible is noise.
    expect(screen.queryByText('public')).toBeNull();
  });
});
