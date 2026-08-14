import { useEffect, useId, useRef, useState } from 'react';
import type { EnvironmentProfile } from '../environment/types';
import './NewSessionDialog.css';

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
  const headingId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const firstFieldRef = useRef<HTMLSelectElement>(null);

  // Focus moves into the dialog on open, or a keyboard user is left behind on
  // the control that opened it with no indication anything happened.
  useEffect(() => {
    if (open) {
      firstFieldRef.current?.focus();
    }
  }, [open]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onCancel();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, onCancel]);

  if (!open) {
    return null;
  }

  return (
    <div className="dialog__scrim" role="presentation" onClick={onCancel}>
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        ref={dialogRef}
        onClick={(event) => {
          // The scrim closes; the panel itself must not.
          event.stopPropagation();
        }}
      >
        <h2 className="dialog__title" id={headingId}>
          New session
        </h2>

        <label className="dialog__field">
          <span className="dialog__label">Environment profile</span>
          <select
            ref={firstFieldRef}
            className="dialog__control"
            value={profile}
            onChange={(e) => {
              setProfile(e.target.value);
            }}
          >
            {/*
             * "None" is not "bare". A session that names no profile is started
             * the way this deployment says an unqualified session should be —
             * which agent, which model, how freely it acts — so this chooses
             * between described sessions rather than between something and
             * nothing.
             */}
            <option value="">This deployment's defaults</option>
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <span className="dialog__hint">
            The whole description of the session: the agent and how it behaves, the software it
            carries, and the variables it is started with.
          </span>
        </label>

        <div className="dialog__actions">
          <button type="button" className="dialog__button" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="dialog__button dialog__button--primary"
            onClick={() => {
              onStart(profile === '' ? null : profile);
            }}
          >
            Start session
          </button>
        </div>
      </div>
    </div>
  );
}
