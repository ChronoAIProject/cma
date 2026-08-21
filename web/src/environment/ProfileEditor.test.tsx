import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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
 * session starts and is not editable, because a session talked out of knowing
 * it spends its first minutes finding out the hard way.
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

  it('says plainly that the platform adds a part the owner cannot edit', () => {
    // Without this the field reads as the whole of what a session is told, and
    // somebody would reasonably write the platform's half themselves.
    editor(emptyDraft());
    const hint = screen.getByText(/the platform adds its own section/i);
    expect(hint).toBeInTheDocument();
    expect(hint.textContent).toMatch(/not editable/i);
  });
});
