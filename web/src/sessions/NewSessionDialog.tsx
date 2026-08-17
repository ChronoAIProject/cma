import { useId, useState } from 'react';
import { Button } from '../ui/Button';
import { Dialog } from '../ui/Dialog';
import { Select } from '../ui/Select';
import type { EnvironmentProfile } from '../environment/types';

/**
 * The choice made when a session starts.
 *
 * Presented once, at creation, because these are fixed for the session's life —
 * the agent is configured before its first turn, and changing them afterwards
 * would mean restarting it and losing the conversation.
 *
 * Deliberately small. Everything has a working default, so someone who wants a
 * terminal can press Start and get one; the settings are there for the person
 * who has a reason to change them.
 *
 * It builds on the shared shell rather than beside it. It used to declare its
 * own scrim, its own plate and its own Escape handler, and that copy trapped no
 * focus at all — Tab out of it landed on the console behind, which is inert to
 * the eye and not to the keyboard.
 */

interface NewSessionDialogProps {
  readonly open: boolean;
  readonly onCancel: () => void;
  /**
   * Start a session from this profile, or from the deployment's own defaults.
   *
   * One argument because a profile is now the whole description of a session:
   * which agent, which model, how freely it acts, and what it carries.
   */
  readonly onStart: (environmentProfile: string | null) => void;
  /**
   * Environment bundles this person can start a session with.
   *
   * Passed in rather than fetched here so the dialog stays a form: it is opened
   * and closed constantly, and fetching on every open would ask the control
   * plane the same question each time.
   */
  readonly profiles?: readonly EnvironmentProfile[];
}

export function NewSessionDialog({
  open,
  onCancel,
  onStart,
  profiles = [],
}: NewSessionDialogProps) {
  const [profile, setProfile] = useState<string>('');
  const profileLabelId = useId();

  if (!open) {
    return null;
  }

  return (
    <Dialog
      title="New session"
      onClose={onCancel}
      footer={
        <>
          <Button onClick={onCancel}>Cancel</Button>
          <Button
            tone="accent"
            onClick={() => {
              onStart(profile === '' ? null : profile);
            }}
          >
            Start session
          </Button>
        </>
      }
    >
      <div className="field">
        <span className="field__label" id={profileLabelId}>
          Environment profile
        </span>
        {/*
         * "None" is not "bare". A session that names no profile is started the
         * way this deployment says an unqualified session should be — which
         * agent, which model, how freely it acts — so this chooses between
         * described sessions rather than between something and nothing.
         */}
        <Select
          label="Environment profile"
          value={profile}
          options={[
            { value: '', label: "This deployment's defaults" },
            ...profiles.map((p) => ({ value: p.id, label: p.name })),
          ]}
          onChange={setProfile}
        />
        <span className="field__hint">
          The whole description of the session: the agent and how it behaves, the software it
          carries, and the variables it is started with.
        </span>
      </div>
    </Dialog>
  );
}
