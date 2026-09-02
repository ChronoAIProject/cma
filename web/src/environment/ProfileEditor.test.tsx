import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { ProfileEditor } from './ProfileEditor';
import { emptyDraft } from './types';
import type { ProfileDraft } from './types';

/**
 * The profile editor's standing-instructions field.
 *
 * What a session is told about itself has two authors and only one of them is
 * here. The platform's half — that the session is already signed in, which
 * services it may reach, which commands will refuse it — is composed when the
 * session starts and controlled separately by a platform administrator.
 */
function editor(draft: ProfileDraft, onChange = vi.fn()) {
  render(
    <ProfileEditor
      draft={draft}
      onChange={onChange}
      kernels={[]}
      skills={[]}
      services={[]}
      mandatoryServices={[]}
      mandatory={{ packages: [], skills: [] }}
    />,
  );
  return onChange;
}

function controlledEditor(initial: ProfileDraft, onChange = vi.fn()) {
  function Harness() {
    const [draft, setDraft] = useState(initial);
    return (
      <ProfileEditor
        draft={draft}
        onChange={(next) => {
          setDraft(next);
          onChange(next);
        }}
        kernels={[]}
        skills={[]}
        services={[]}
        mandatoryServices={[]}
        mandatory={{ packages: [], skills: [] }}
      />
    );
  }

  render(<Harness />);
  return onChange;
}

describe('session resources', () => {
  it('shows the machine specification while leaving deployment defaults unset', () => {
    controlledEditor(emptyDraft());

    expect(screen.getByRole('checkbox', { name: /use deployment default/i })).toBeChecked();
    const cpu = screen.getByLabelText(/cpu \(millicores\)/i);
    const memory = screen.getByLabelText(/memory \(mib\)/i);
    expect(cpu).toBeDisabled();
    expect(memory).toBeDisabled();
    expect(cpu).toHaveAttribute('placeholder', 'Deployment default');
    expect(memory).toHaveAttribute('placeholder', 'Deployment default');
  });

  it('carries a complete custom size in the profile draft', async () => {
    const user = userEvent.setup();
    const onChange = controlledEditor(emptyDraft());

    await user.click(screen.getByRole('checkbox', { name: /use deployment default/i }));
    await user.type(screen.getByLabelText(/cpu \(millicores\)/i), '2000');
    await user.type(screen.getByLabelText(/memory \(mib\)/i), '8192');

    const changed = onChange.mock.calls.at(-1)?.[0] as ProfileDraft;
    expect(changed.agent.size).toEqual({ cpuMillis: 2000, memoryMib: 8192 });
  });

  it('removes a custom size and restores it when the deployment default is toggled twice', async () => {
    const user = userEvent.setup();
    const draft = emptyDraft();
    const onChange = controlledEditor({
      ...draft,
      agent: { ...draft.agent, size: { cpuMillis: 1500, memoryMib: 3072 } },
    });
    const deploymentDefault = screen.getByRole('checkbox', { name: /use deployment default/i });

    expect(deploymentDefault).not.toBeChecked();
    expect(screen.getByLabelText(/cpu \(millicores\)/i)).toHaveValue(1500);
    expect(screen.getByLabelText(/memory \(mib\)/i)).toHaveValue(3072);

    await user.click(deploymentDefault);
    expect((onChange.mock.calls.at(-1)?.[0] as ProfileDraft).agent.size).toBeUndefined();
    expect(screen.getByLabelText(/cpu \(millicores\)/i)).toBeDisabled();
    expect(screen.getByLabelText(/memory \(mib\)/i)).toBeDisabled();

    await user.click(deploymentDefault);
    expect(screen.getByLabelText(/cpu \(millicores\)/i)).toHaveValue(1500);
    expect(screen.getByLabelText(/memory \(mib\)/i)).toHaveValue(3072);
  });
});

describe('standing instructions', () => {
  it('offers a field for what the owner wants every session to start with', () => {
    editor(emptyDraft());
    expect(screen.getByLabelText(/standing instructions/i)).toBeInTheDocument();
  });

  it('carries what was typed back to the caller', async () => {
    const user = userEvent.setup();
    const onChange = editor(emptyDraft());
    await user.type(screen.getByLabelText(/standing instructions/i), 'Run the tests.');
    // Controlled input: each keystroke reports the whole draft, and the last
    // one carries the last character rather than the whole string.
    expect(onChange).toHaveBeenCalled();
    const last = onChange.mock.calls.at(-1)?.[0] as ProfileDraft;
    expect(last.agentNotes).toBe('.');
  });

  it('shows what the owner already wrote', () => {
    editor({ ...emptyDraft(), agentNotes: 'Always prefer small commits.' });
    expect(screen.getByLabelText(/standing instructions/i)).toHaveValue(
      'Always prefer small commits.',
    );
  });

  it('says plainly that platform administrators control the platform section', () => {
    // Without this the field reads as the whole of what a session is told, and
    // somebody would reasonably write the platform's half themselves.
    editor(emptyDraft());
    const hint = screen.getByText(/the platform adds its own section/i);
    expect(hint).toBeInTheDocument();
    expect(hint.textContent).toMatch(/platform administrators control/i);
    expect(hint.textContent).toMatch(/only your instructions/i);
  });
});
