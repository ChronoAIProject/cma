import { useEffect, useId, useRef, useState } from 'react';
import type { EnvironmentProfile } from '../environment/types';
import {
  CREDENTIALS,
  DEFAULT_OPTIONS,
  EFFORTS,
  KERNELS,
  PERMISSIONS,
  isDangerous,
  type AgentOptions,
  type CredentialMode,
  type PermissionMode,
  type ReasoningEffort,
} from './options';
import './NewSessionDialog.css';

/**
 * The choices made when a session starts.
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
  readonly onStart: (
    kernel: string,
    options: AgentOptions,
    environmentProfile: string | null,
  ) => void;
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
  const [kernel, setKernel] = useState('codex');
  const [options, setOptions] = useState<AgentOptions>(DEFAULT_OPTIONS);
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

  const dangerous = isDangerous(options.permission);

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
          <span className="dialog__label">Agent</span>
          <select
            ref={firstFieldRef}
            className="dialog__control"
            value={kernel}
            onChange={(e) => {
              setKernel(e.target.value);
            }}
          >
            {KERNELS.map((k) => (
              <option key={k.id} value={k.id} disabled={k.unavailable !== undefined}>
                {k.unavailable === undefined ? k.label : `${k.label} — ${k.unavailable}`}
              </option>
            ))}
          </select>
        </label>

        <label className="dialog__field">
          <span className="dialog__label">Model</span>
          <input
            className="dialog__control"
            type="text"
            placeholder="Default for this deployment"
            value={options.model ?? ''}
            onChange={(e) => {
              const model = e.target.value.trim();
              setOptions((o) => ({ ...o, model: model === '' ? undefined : model }));
            }}
          />
          <span className="dialog__hint">Leave empty to use the configured default</span>
        </label>

        <label className="dialog__field">
          <span className="dialog__label">Thinking effort</span>
          <select
            className="dialog__control"
            value={options.reasoning_effort}
            onChange={(e) => {
              setOptions((o) => ({ ...o, reasoning_effort: e.target.value as ReasoningEffort }));
            }}
          >
            {EFFORTS.map((eff) => (
              <option key={eff.id} value={eff.id}>
                {eff.hint === '' ? eff.label : `${eff.label} — ${eff.hint}`}
              </option>
            ))}
          </select>
        </label>

        <label className="dialog__field">
          <span className="dialog__label">Permissions</span>
          <select
            className="dialog__control"
            value={options.permission}
            onChange={(e) => {
              setOptions((o) => ({ ...o, permission: e.target.value as PermissionMode }));
            }}
          >
            {PERMISSIONS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
          <span className="dialog__hint">
            {PERMISSIONS.find((p) => p.id === options.permission)?.hint}
          </span>
        </label>

        {dangerous ? (
          // Stated plainly rather than hidden behind a confirmation step. The
          // person chose this deliberately; what they need is to know what it
          // means, not another click.
          <p className="dialog__warning" role="alert">
            This agent will run commands without asking and is not confined within its sandbox. Use
            it for work you would be comfortable running yourself.
          </p>
        ) : null}

        <label className="dialog__field">
          <span className="dialog__label">Model access</span>
          <select
            className="dialog__control"
            value={options.credential}
            onChange={(e) => {
              setOptions((o) => ({ ...o, credential: e.target.value as CredentialMode }));
            }}
          >
            {CREDENTIALS.map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
          <span className="dialog__hint">
            {CREDENTIALS.find((c) => c.id === options.credential)?.hint}
          </span>
        </label>

        {profiles.length > 0 ? (
          <label className="dialog__field">
            <span className="dialog__label">Environment</span>
            <select
              className="dialog__control"
              value={profile}
              onChange={(e) => {
                setProfile(e.target.value);
              }}
            >
              {/*
               * "None" is not "bare". What the platform mandates is applied
               * whichever option is chosen, so this selects what to add rather
               * than whether to have anything at all.
               */}
              <option value="">None beyond the platform defaults</option>
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <span className="dialog__hint">
              Tools, variables and skills the session starts with.
            </span>
          </label>
        ) : null}

        <div className="dialog__actions">
          <button type="button" className="dialog__button" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="dialog__button dialog__button--primary"
            onClick={() => {
              onStart(kernel, options, profile === '' ? null : profile);
            }}
          >
            Start session
          </button>
        </div>
      </div>
    </div>
  );
}
