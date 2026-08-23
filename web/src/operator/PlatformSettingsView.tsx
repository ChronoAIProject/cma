import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import {
  BriefingField,
  MAX_U32,
  NumberField,
  ProseBlocks,
  RangeFields,
} from './PlatformSettingsFields';
import {
  PlatformSettingsConflict,
  type OperatorStore,
  type PlatformSettings,
  type SessionBriefing,
  type StoredPlatformSettings,
} from './types';
import './PlatformSettingsView.css';

interface PlatformSettingsViewProps {
  readonly store: OperatorStore;
}

type EditorState =
  | { readonly kind: 'loading' }
  | {
      readonly kind: 'ready';
      readonly revision: number;
      readonly current: PlatformSettings;
      readonly previous?: PlatformSettings;
      readonly draft: PlatformSettings;
    };

type ReadyEditorState = Extract<EditorState, { readonly kind: 'ready' }>;

interface ConflictState {
  readonly message: string;
  readonly current: StoredPlatformSettings;
  readonly attempted: 'save' | 'reset';
}

interface Validation {
  readonly deploymentCpu?: string;
  readonly deploymentMemory?: string;
  readonly assumedCpu?: string;
  readonly assumedMemory?: string;
  readonly cpu?: string;
  readonly memory?: string;
  readonly liveSessions?: string;
}

function copyBriefing(briefing: SessionBriefing): SessionBriefing {
  return { ...briefing, before: [...briefing.before], after: [...briefing.after] };
}

function copySettings(settings: PlatformSettings): PlatformSettings {
  return {
    briefing: copyBriefing(settings.briefing),
    deploymentCapacity: {
      ...settings.deploymentCapacity,
      assumedSessionSize:
        settings.deploymentCapacity.assumedSessionSize === null
          ? null
          : { ...settings.deploymentCapacity.assumedSessionSize },
    },
    sizeBounds: { ...settings.sizeBounds },
    liveSessions: settings.liveSessions,
  };
}

function readyFrom(stored: StoredPlatformSettings): ReadyEditorState {
  return {
    kind: 'ready',
    revision: stored.revision,
    current: copySettings(stored.settings),
    previous: stored.previous === undefined ? undefined : copySettings(stored.previous),
    draft: copySettings(stored.settings),
  };
}

function sameBriefing(left: SessionBriefing, right: SessionBriefing): boolean {
  return (
    left.reachHeading === right.reachHeading &&
    left.reachLead === right.reachLead &&
    left.reachClose === right.reachClose &&
    left.reachNone === right.reachNone &&
    left.before.length === right.before.length &&
    left.before.every((block, index) => block === right.before[index]) &&
    left.after.length === right.after.length &&
    left.after.every((block, index) => block === right.after[index])
  );
}

function sameSettings(left: PlatformSettings, right: PlatformSettings): boolean {
  const leftAssumed = left.deploymentCapacity.assumedSessionSize;
  const rightAssumed = right.deploymentCapacity.assumedSessionSize;
  return (
    sameBriefing(left.briefing, right.briefing) &&
    left.deploymentCapacity.cpuMillis === right.deploymentCapacity.cpuMillis &&
    left.deploymentCapacity.memoryMib === right.deploymentCapacity.memoryMib &&
    (leftAssumed === null
      ? rightAssumed === null
      : rightAssumed !== null &&
        leftAssumed.cpuMillis === rightAssumed.cpuMillis &&
        leftAssumed.memoryMib === rightAssumed.memoryMib) &&
    left.sizeBounds.minCpuMillis === right.sizeBounds.minCpuMillis &&
    left.sizeBounds.maxCpuMillis === right.sizeBounds.maxCpuMillis &&
    left.sizeBounds.minMemoryMib === right.sizeBounds.minMemoryMib &&
    left.sizeBounds.maxMemoryMib === right.sizeBounds.maxMemoryMib &&
    left.liveSessions === right.liveSessions
  );
}

function deploymentNumberProblem(value: number): string | undefined {
  return Number.isSafeInteger(value) && value >= 0
    ? undefined
    : 'Use a whole number from 0 to 9,007,199,254,740,991.';
}

function numberProblem(value: number): string | undefined {
  return Number.isSafeInteger(value) && value > 0 && value <= MAX_U32
    ? undefined
    : 'Use a whole number from 1 to 4,294,967,295.';
}

export function validatePlatformSettings(settings: PlatformSettings): Validation {
  const assumed = settings.deploymentCapacity.assumedSessionSize;
  const cpuNumber =
    numberProblem(settings.sizeBounds.minCpuMillis) ??
    numberProblem(settings.sizeBounds.maxCpuMillis);
  const memoryNumber =
    numberProblem(settings.sizeBounds.minMemoryMib) ??
    numberProblem(settings.sizeBounds.maxMemoryMib);
  return {
    deploymentCpu:
      settings.deploymentCapacity.cpuMillis === null
        ? undefined
        : deploymentNumberProblem(settings.deploymentCapacity.cpuMillis),
    deploymentMemory:
      settings.deploymentCapacity.memoryMib === null
        ? undefined
        : deploymentNumberProblem(settings.deploymentCapacity.memoryMib),
    assumedCpu: assumed === null ? undefined : numberProblem(assumed.cpuMillis),
    assumedMemory: assumed === null ? undefined : numberProblem(assumed.memoryMib),
    cpu:
      cpuNumber ??
      (settings.sizeBounds.minCpuMillis > settings.sizeBounds.maxCpuMillis
        ? 'Minimum CPU cannot exceed maximum CPU.'
        : undefined),
    memory:
      memoryNumber ??
      (settings.sizeBounds.minMemoryMib > settings.sizeBounds.maxMemoryMib
        ? 'Minimum memory cannot exceed maximum memory.'
        : undefined),
    liveSessions: settings.liveSessions === null ? undefined : numberProblem(settings.liveSessions),
  };
}

function validationMessage(validation: Validation): string | undefined {
  return (
    validation.deploymentCpu ??
    validation.deploymentMemory ??
    validation.assumedCpu ??
    validation.assumedMemory ??
    validation.cpu ??
    validation.memory ??
    validation.liveSessions
  );
}

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function PlatformSettingsView({ store }: PlatformSettingsViewProps) {
  const id = useId();
  const [editor, setEditor] = useState<EditorState>({ kind: 'loading' });
  const [operation, setOperation] = useState<'save' | 'reset' | undefined>(undefined);
  const [error, setError] = useState<string | undefined>(undefined);
  const [status, setStatus] = useState<string | undefined>(undefined);
  const [conflict, setConflict] = useState<ConflictState | undefined>(undefined);
  const [validation, setValidation] = useState<Validation>({});
  const [readAttempt, setReadAttempt] = useState(0);
  const lastLiveLimit = useRef(1);
  const lastDeploymentCpu = useRef(0);
  const lastDeploymentMemory = useRef(0);
  const lastAssumedCpu = useRef(1);
  const lastAssumedMemory = useRef(1);

  const busy = operation !== undefined;

  const rememberCapacity = useCallback((settings: PlatformSettings): void => {
    if (settings.deploymentCapacity.cpuMillis !== null) {
      lastDeploymentCpu.current = settings.deploymentCapacity.cpuMillis;
    }
    if (settings.deploymentCapacity.memoryMib !== null) {
      lastDeploymentMemory.current = settings.deploymentCapacity.memoryMib;
    }
    const assumed = settings.deploymentCapacity.assumedSessionSize;
    if (assumed !== null) {
      lastAssumedCpu.current = assumed.cpuMillis;
      lastAssumedMemory.current = assumed.memoryMib;
    }
  }, []);

  useEffect(() => {
    let mounted = true;
    store
      .readPlatformSettings()
      .then((stored) => {
        if (mounted) {
          if (stored.settings.liveSessions !== null) {
            lastLiveLimit.current = stored.settings.liveSessions;
          }
          rememberCapacity(stored.settings);
          setEditor(readyFrom(stored));
        }
      })
      .catch((cause: unknown) => {
        if (mounted) {
          setError(errorText(cause));
        }
      });
    return () => {
      mounted = false;
    };
  }, [readAttempt, rememberCapacity, store]);

  function changeDraft(change: (draft: PlatformSettings) => PlatformSettings): void {
    setEditor((state) =>
      state.kind === 'ready' ? { ...state, draft: change(state.draft) } : state,
    );
    setValidation({});
    setStatus(undefined);
  }

  function accept(
    stored: StoredPlatformSettings,
    message: string,
    draft: PlatformSettings = stored.settings,
  ): void {
    if (draft.liveSessions !== null) {
      lastLiveLimit.current = draft.liveSessions;
    }
    rememberCapacity(draft);
    setEditor({ ...readyFrom(stored), draft: copySettings(draft) });
    setConflict(undefined);
    setError(undefined);
    setValidation({});
    setStatus(message);
  }

  async function recoverConflict(
    cause: PlatformSettingsConflict,
    attempted: ConflictState['attempted'],
  ): Promise<void> {
    try {
      const current = await store.readPlatformSettings();
      setEditor((state) =>
        state.kind === 'ready'
          ? {
              ...state,
              revision: current.revision,
              current: copySettings(current.settings),
              previous: current.previous === undefined ? undefined : copySettings(current.previous),
            }
          : state,
      );
      setConflict({ message: cause.message, current, attempted });
    } catch (readCause) {
      setError(`${cause.message} The current settings could not be read: ${errorText(readCause)}`);
    }
  }

  async function save(): Promise<void> {
    if (editor.kind !== 'ready' || busy) {
      return;
    }
    const nextValidation = validatePlatformSettings(editor.draft);
    const problem = validationMessage(nextValidation);
    if (problem !== undefined) {
      setValidation(nextValidation);
      setError(undefined);
      return;
    }
    setOperation('save');
    setError(undefined);
    setStatus(undefined);
    setConflict(undefined);
    try {
      const stored = await store.changePlatformSettings(editor.revision, editor.draft);
      accept(stored, `Saved revision ${String(stored.revision)}.`);
    } catch (cause) {
      if (cause instanceof PlatformSettingsConflict) {
        await recoverConflict(cause, 'save');
      } else {
        setError(errorText(cause));
      }
    } finally {
      setOperation(undefined);
    }
  }

  async function resetBriefing(): Promise<void> {
    if (editor.kind !== 'ready' || busy) {
      return;
    }
    setOperation('reset');
    setError(undefined);
    setStatus(undefined);
    setConflict(undefined);
    try {
      const stored = await store.resetPlatformBriefing(editor.revision);
      accept(stored, `Restored shipped briefing in revision ${String(stored.revision)}.`, {
        ...editor.draft,
        briefing: copyBriefing(stored.settings.briefing),
      });
    } catch (cause) {
      if (cause instanceof PlatformSettingsConflict) {
        await recoverConflict(cause, 'reset');
      } else {
        setError(errorText(cause));
      }
    } finally {
      setOperation(undefined);
    }
  }

  if (editor.kind === 'loading') {
    return error === undefined ? (
      <p className="operator__waiting">Reading platform settings…</p>
    ) : (
      <div className="notice notice--fail platform-settings__read-error" role="alert">
        <div>
          <strong className="notice__title">Platform settings could not be read</strong>
          <p className="notice__detail">{error}</p>
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            setError(undefined);
            setReadAttempt((attempt) => attempt + 1);
          }}
        >
          Try again
        </Button>
      </div>
    );
  }

  const { draft } = editor;
  const dirty = !sameSettings(draft, editor.current);

  return (
    <form
      className="platform-settings"
      aria-label="Platform settings"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div className="platform-settings__heading">
        <div>
          <h2 className="platform-settings__title">Platform settings</h2>
          <p className="platform-settings__revision">Revision {String(editor.revision)}</p>
        </div>
        {editor.previous === undefined ? null : (
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              const previous = editor.previous as PlatformSettings;
              if (previous.liveSessions !== null) {
                lastLiveLimit.current = previous.liveSessions;
              }
              rememberCapacity(previous);
              changeDraft(() => copySettings(previous));
              setStatus('Previous values loaded into the draft. Save changes to write them.');
            }}
          >
            Use previous values
          </Button>
        )}
      </div>

      <div className="notice notice--quiet platform-settings__notice">
        <p className="notice__detail">
          Only Codex currently reads the session briefing. Clearing its prose removes the guidance
          that a session is already authenticated and that <code>nyxid login</code> will fail.
        </p>
      </div>

      {conflict === undefined ? null : (
        <div className="notice notice--quiet platform-settings__conflict" role="alert">
          <div className="platform-settings__conflict-copy">
            <strong className="notice__title">Settings changed elsewhere</strong>
            <p className="notice__detail">
              {conflict.message} Your draft is unchanged. Revision{' '}
              {String(conflict.current.revision)} is current. Load its values, or review the
              retained draft and {conflict.attempted === 'reset' ? 'reset' : 'save'} again against
              that revision.
            </p>
          </div>
          <div className="platform-settings__conflict-actions">
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                if (conflict.current.settings.liveSessions !== null) {
                  lastLiveLimit.current = conflict.current.settings.liveSessions;
                }
                rememberCapacity(conflict.current.settings);
                setEditor(readyFrom(conflict.current));
                setConflict(undefined);
                setError(undefined);
                setStatus('Loaded the current platform settings.');
              }}
            >
              Load current values
            </Button>
            <Button
              size="sm"
              onClick={() => {
                setConflict(undefined);
                setStatus(`Draft retained against revision ${String(conflict.current.revision)}.`);
              }}
            >
              Review retained draft
            </Button>
          </div>
        </div>
      )}

      {error === undefined ? null : (
        <p className="field__error platform-settings__error" role="alert">
          {error}
        </p>
      )}
      {status === undefined ? null : (
        <p className="platform-settings__status" role="status">
          {status}
        </p>
      )}

      <fieldset className="platform-settings__controls" disabled={busy}>
        <section className="platform-settings__section" aria-labelledby={`${id}-briefing`}>
          <div className="platform-settings__section-head">
            <h3 id={`${id}-briefing`} className="platform-settings__section-title">
              Session briefing
            </h3>
            <p className="platform-settings__section-note">
              Applied to new Codex sessions before the owner’s standing instructions.
            </p>
          </div>

          <ProseBlocks
            label="Prose before granted services"
            empty="No platform prose appears before the granted-services section."
            blocks={draft.briefing.before}
            onChange={(before) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, before },
              }));
            }}
          />

          <BriefingField
            id={`${id}-reach-heading`}
            label="Granted-services heading"
            value={draft.briefing.reachHeading}
            onChange={(reachHeading) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, reachHeading },
              }));
            }}
          />
          <BriefingField
            id={`${id}-reach-lead`}
            label="Introduction when services are granted"
            value={draft.briefing.reachLead}
            multiline
            onChange={(reachLead) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, reachLead },
              }));
            }}
          />

          <div
            className="platform-settings__generated"
            aria-label="Generated granted-services list"
          >
            <div>
              <span className="platform-settings__generated-title">Granted-service slugs</span>
              <p className="platform-settings__generated-detail">
                Generated from the services granted to each session.
              </p>
            </div>
            <span className="platform-settings__fixed">Fixed</span>
          </div>

          <BriefingField
            id={`${id}-reach-close`}
            label="Closing prose when services are granted"
            value={draft.briefing.reachClose}
            multiline
            onChange={(reachClose) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, reachClose },
              }));
            }}
          />
          <BriefingField
            id={`${id}-reach-none`}
            label="Prose when no services are granted"
            value={draft.briefing.reachNone}
            multiline
            onChange={(reachNone) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, reachNone },
              }));
            }}
          />

          <ProseBlocks
            label="Prose after granted services"
            empty="No platform prose appears after the granted-services section."
            blocks={draft.briefing.after}
            onChange={(after) => {
              changeDraft((settings) => ({
                ...settings,
                briefing: { ...settings.briefing, after },
              }));
            }}
          />
        </section>

        <section
          className="platform-settings__section"
          aria-labelledby={`${id}-deployment-capacity`}
        >
          <div className="platform-settings__section-head">
            <h3 id={`${id}-deployment-capacity`} className="platform-settings__section-title">
              Deployment capacity
            </h3>
            <p className="platform-settings__section-note">
              Declared totals reserved across sessions that want a runtime.
            </p>
          </div>

          <div className="platform-settings__ranges">
            <div className="platform-settings__range">
              <label className="platform-settings__toggle">
                <input
                  type="checkbox"
                  checked={draft.deploymentCapacity.cpuMillis !== null}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    changeDraft((settings) => {
                      const current = settings.deploymentCapacity.cpuMillis;
                      if (!checked && current !== null) {
                        lastDeploymentCpu.current = current;
                      }
                      return {
                        ...settings,
                        deploymentCapacity: {
                          ...settings.deploymentCapacity,
                          cpuMillis: checked ? lastDeploymentCpu.current : null,
                        },
                      };
                    });
                  }}
                />
                Declare a CPU total
              </label>
              {draft.deploymentCapacity.cpuMillis === null ? null : (
                <NumberField
                  id={`${id}-deployment-cpu`}
                  label="Total CPU"
                  value={draft.deploymentCapacity.cpuMillis}
                  minimum={0}
                  maximum={Number.MAX_SAFE_INTEGER}
                  error={validation.deploymentCpu}
                  onChange={(cpuMillis) => {
                    if (deploymentNumberProblem(cpuMillis) === undefined) {
                      lastDeploymentCpu.current = cpuMillis;
                    }
                    changeDraft((settings) => ({
                      ...settings,
                      deploymentCapacity: { ...settings.deploymentCapacity, cpuMillis },
                    }));
                  }}
                />
              )}
              {validation.deploymentCpu === undefined ? null : (
                <p className="field__error" role="alert">
                  {validation.deploymentCpu}
                </p>
              )}
            </div>

            <div className="platform-settings__range">
              <label className="platform-settings__toggle">
                <input
                  type="checkbox"
                  checked={draft.deploymentCapacity.memoryMib !== null}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    changeDraft((settings) => {
                      const current = settings.deploymentCapacity.memoryMib;
                      if (!checked && current !== null) {
                        lastDeploymentMemory.current = current;
                      }
                      return {
                        ...settings,
                        deploymentCapacity: {
                          ...settings.deploymentCapacity,
                          memoryMib: checked ? lastDeploymentMemory.current : null,
                        },
                      };
                    });
                  }}
                />
                Declare a memory total
              </label>
              {draft.deploymentCapacity.memoryMib === null ? null : (
                <NumberField
                  id={`${id}-deployment-memory`}
                  label="Total memory (MiB)"
                  value={draft.deploymentCapacity.memoryMib}
                  minimum={0}
                  maximum={Number.MAX_SAFE_INTEGER}
                  error={validation.deploymentMemory}
                  onChange={(memoryMib) => {
                    if (deploymentNumberProblem(memoryMib) === undefined) {
                      lastDeploymentMemory.current = memoryMib;
                    }
                    changeDraft((settings) => ({
                      ...settings,
                      deploymentCapacity: { ...settings.deploymentCapacity, memoryMib },
                    }));
                  }}
                />
              )}
              {validation.deploymentMemory === undefined ? null : (
                <p className="field__error" role="alert">
                  {validation.deploymentMemory}
                </p>
              )}
            </div>
          </div>

          <label className="platform-settings__toggle">
            <input
              type="checkbox"
              checked={draft.deploymentCapacity.assumedSessionSize !== null}
              onChange={(event) => {
                const checked = event.target.checked;
                changeDraft((settings) => {
                  const current = settings.deploymentCapacity.assumedSessionSize;
                  if (!checked && current !== null) {
                    lastAssumedCpu.current = current.cpuMillis;
                    lastAssumedMemory.current = current.memoryMib;
                  }
                  return {
                    ...settings,
                    deploymentCapacity: {
                      ...settings.deploymentCapacity,
                      assumedSessionSize: checked
                        ? {
                            cpuMillis: lastAssumedCpu.current,
                            memoryMib: lastAssumedMemory.current,
                          }
                        : null,
                    },
                  };
                });
              }}
            />
            Account for sessions without a recorded size
          </label>

          {draft.deploymentCapacity.assumedSessionSize === null ? null : (
            <>
              <div className="platform-settings__ranges">
                <NumberField
                  id={`${id}-assumed-cpu`}
                  label="Assumed CPU (millicores)"
                  value={draft.deploymentCapacity.assumedSessionSize.cpuMillis}
                  error={validation.assumedCpu}
                  onChange={(cpuMillis) => {
                    if (numberProblem(cpuMillis) === undefined) {
                      lastAssumedCpu.current = cpuMillis;
                    }
                    changeDraft((settings) => {
                      const assumed = settings.deploymentCapacity.assumedSessionSize;
                      return assumed === null
                        ? settings
                        : {
                            ...settings,
                            deploymentCapacity: {
                              ...settings.deploymentCapacity,
                              assumedSessionSize: { ...assumed, cpuMillis },
                            },
                          };
                    });
                  }}
                />
                <NumberField
                  id={`${id}-assumed-memory`}
                  label="Assumed memory (MiB)"
                  value={draft.deploymentCapacity.assumedSessionSize.memoryMib}
                  error={validation.assumedMemory}
                  onChange={(memoryMib) => {
                    if (numberProblem(memoryMib) === undefined) {
                      lastAssumedMemory.current = memoryMib;
                    }
                    changeDraft((settings) => {
                      const assumed = settings.deploymentCapacity.assumedSessionSize;
                      return assumed === null
                        ? settings
                        : {
                            ...settings,
                            deploymentCapacity: {
                              ...settings.deploymentCapacity,
                              assumedSessionSize: { ...assumed, memoryMib },
                            },
                          };
                    });
                  }}
                />
              </div>
              {validation.assumedCpu === undefined &&
              validation.assumedMemory === undefined ? null : (
                <p className="field__error" role="alert">
                  {validation.assumedCpu ?? validation.assumedMemory}
                </p>
              )}
            </>
          )}

          {(draft.deploymentCapacity.cpuMillis !== null ||
            draft.deploymentCapacity.memoryMib !== null) &&
          draft.deploymentCapacity.assumedSessionSize === null ? (
            <p className="field__hint platform-settings__capacity-warning">
              Capacity admission is inactive for a session without a recorded size, or while any
              existing session lacks one.
            </p>
          ) : null}
        </section>

        <section className="platform-settings__section" aria-labelledby={`${id}-capacity`}>
          <div className="platform-settings__section-head">
            <h3 id={`${id}-capacity`} className="platform-settings__section-title">
              Session size bounds
            </h3>
            <p className="platform-settings__section-note">
              Inclusive limits for CPU and memory a new session states explicitly.
            </p>
          </div>
          <div className="platform-settings__ranges">
            <RangeFields
              id={`${id}-cpu`}
              label="CPU"
              unit="millicores"
              minimum={draft.sizeBounds.minCpuMillis}
              maximum={draft.sizeBounds.maxCpuMillis}
              error={validation.cpu}
              onChange={(minimum, maximum) => {
                changeDraft((settings) => ({
                  ...settings,
                  sizeBounds: {
                    ...settings.sizeBounds,
                    minCpuMillis: minimum,
                    maxCpuMillis: maximum,
                  },
                }));
              }}
            />
            <RangeFields
              id={`${id}-memory`}
              label="Memory"
              unit="MiB"
              minimum={draft.sizeBounds.minMemoryMib}
              maximum={draft.sizeBounds.maxMemoryMib}
              error={validation.memory}
              onChange={(minimum, maximum) => {
                changeDraft((settings) => ({
                  ...settings,
                  sizeBounds: {
                    ...settings.sizeBounds,
                    minMemoryMib: minimum,
                    maxMemoryMib: maximum,
                  },
                }));
              }}
            />
          </div>
        </section>

        <section className="platform-settings__section" aria-labelledby={`${id}-concurrency`}>
          <div className="platform-settings__section-head">
            <h3 id={`${id}-concurrency`} className="platform-settings__section-title">
              Live sessions
            </h3>
            <p className="platform-settings__section-note">
              A per-person limit for new and resumed sessions. Existing work is never stopped.
            </p>
          </div>
          <label className="platform-settings__toggle">
            <input
              type="checkbox"
              checked={draft.liveSessions !== null}
              onChange={(event) => {
                const checked = event.target.checked;
                changeDraft((settings) => {
                  if (!checked && settings.liveSessions !== null) {
                    lastLiveLimit.current = settings.liveSessions;
                  }
                  return {
                    ...settings,
                    liveSessions: checked ? lastLiveLimit.current : null,
                  };
                });
              }}
            />
            Limit live sessions per person
          </label>
          {draft.liveSessions === null ? null : (
            <NumberField
              id={`${id}-live-sessions`}
              label="Maximum live sessions"
              value={draft.liveSessions}
              error={validation.liveSessions}
              onChange={(liveSessions) => {
                if (numberProblem(liveSessions) === undefined) {
                  lastLiveLimit.current = liveSessions;
                }
                changeDraft((settings) => ({ ...settings, liveSessions }));
              }}
            />
          )}
        </section>

        <div className="platform-settings__actions">
          <Button variant="primary" type="submit" disabled={busy || !dirty}>
            {operation === 'save' ? 'Saving…' : 'Save changes'}
          </Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              void resetBriefing();
            }}
          >
            {operation === 'reset' ? 'Resetting…' : 'Reset briefing'}
          </Button>
          {!dirty ? <span className="platform-settings__unchanged">No unsaved changes</span> : null}
        </div>
      </fieldset>
    </form>
  );
}
