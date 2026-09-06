import { useEffect, useRef, useState } from 'react';
import { CommandKeys, type CommandOptions } from '../api/client';
import type {
  ProfileDocument,
  ProfileDraft,
  ProfileEditorClient,
  ProfileVisibility,
} from './client';
import { profileFailure, type ProfileFailure } from './failure';
import type { FieldErrors } from './validation';

export type ProfileCommand = 'save' | 'publish' | 'visibility' | 'retire';

function draftOf<Content>(profile: ProfileDocument<Content>): ProfileDraft<Content> {
  return { name: profile.name, description: profile.description, content: profile.draft };
}

interface Transaction<Content> {
  readonly kind: ProfileCommand;
  readonly scope: string;
  readonly execute: (options: CommandOptions) => Promise<ProfileDocument<Content>>;
}

export function useProfileEditor<Content>(
  client: ProfileEditorClient<Content>,
  profileId: string | undefined,
  initial: ProfileDraft<Content>,
  validate: (draft: ProfileDraft<Content>) => FieldErrors,
  onSaved?: (profile: ProfileDocument<Content>) => void,
) {
  const [profile, setProfile] = useState<ProfileDocument<Content>>();
  const [draft, setDraft] = useState(initial);
  const [loading, setLoading] = useState(profileId !== undefined);
  const [readingCurrent, setReadingCurrent] = useState(false);
  const [readError, setReadError] = useState<string>();
  const [failure, setFailure] = useState<ProfileFailure>();
  const [current, setCurrent] = useState<ProfileDocument<Content>>();
  const [pending, setPending] = useState<ProfileCommand>();
  const [unresolved, setUnresolved] = useState(false);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [validationAttempt, setValidationAttempt] = useState(0);
  const [success, setSuccess] = useState<string>();
  const [completed, setCompleted] = useState<{ record: ProfileDocument<Content> }>();
  const [reload, setReload] = useState(0);
  const keys = useRef(new CommandKeys());
  const transaction = useRef<Transaction<Content> | undefined>(undefined);
  const abort = useRef<AbortController | undefined>(undefined);
  const readAbort = useRef<AbortController | undefined>(undefined);
  const mounted = useRef(true);
  const submitted = useRef(false);
  const savedCallback = useRef(onSaved);
  useEffect(() => {
    savedCallback.current = onSaved;
  }, [onSaved]);
  useEffect(() => {
    if (completed !== undefined) savedCallback.current?.(completed.record);
  }, [completed]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      abort.current?.abort();
      readAbort.current?.abort();
    };
  }, []);

  useEffect(() => {
    if (profileId === undefined) return;
    const request = new AbortController();
    void client
      .readProfile(profileId, { signal: request.signal })
      .then((record) => {
        if (request.signal.aborted) return;
        if (record.id !== profileId) throw new Error('Profile identity mismatch');
        setProfile(record);
        setDraft(draftOf(record));
        setLoading(false);
        setReadError(undefined);
      })
      .catch(() => {
        if (request.signal.aborted) return;
        setLoading(false);
        setReadError('This profile could not be loaded.');
      });
    return () => request.abort();
  }, [client, profileId, reload]);

  const dirty =
    JSON.stringify(draft) !== JSON.stringify(profile === undefined ? initial : draftOf(profile));
  const locked =
    loading ||
    readingCurrent ||
    pending !== undefined ||
    unresolved ||
    profile?.retiredAt !== undefined;

  const change = (next: ProfileDraft<Content>) => {
    if (locked) return;
    setDraft(next);
    setSuccess(undefined);
    if (submitted.current) setErrors(validate(next));
    if (failure?.conflict !== true) setFailure(undefined);
  };

  const run = async (command: Transaction<Content>) => {
    if (abort.current !== undefined || !mounted.current) return;
    const request = new AbortController();
    abort.current = request;
    transaction.current = command;
    setPending(command.kind);
    setUnresolved(false);
    setFailure(undefined);
    setSuccess(undefined);
    try {
      const record = await command.execute({
        idempotencyKey: keys.current.for(command.scope),
        signal: request.signal,
      });
      if (!mounted.current) return;
      setProfile(record);
      setDraft(draftOf(record));
      setErrors({});
      setCurrent(undefined);
      keys.current.settle(command.scope);
      transaction.current = undefined;
      setSuccess(
        command.kind === 'publish'
          ? `Revision ${String(record.latestRevision?.revision)} published.`
          : command.kind === 'retire'
            ? 'Profile retired.'
            : command.kind === 'visibility'
              ? 'Sharing updated.'
              : 'Draft saved.',
      );
      setCompleted({ record });
    } catch (error: unknown) {
      if (!mounted.current) return;
      const problem = profileFailure(error);
      setFailure(problem);
      setErrors(problem.fields);
      setUnresolved(problem.uncertain);
      if (!problem.uncertain) {
        keys.current.settle(command.scope);
        transaction.current = undefined;
      }
      if (problem.conflict && profile !== undefined) {
        try {
          const latest = await client.readProfile(profile.id, { signal: request.signal });
          if (mounted.current && latest.id === profile.id) setCurrent(latest);
        } catch {
          /* The draft remains available; the explicit retry can reread. */
        }
      }
    } finally {
      abort.current = undefined;
      if (mounted.current) setPending(undefined);
    }
  };

  const save = (basis = profile) => {
    if (locked || (failure?.conflict && basis === profile)) return;
    submitted.current = true;
    const invalid = validate(draft);
    setErrors(invalid);
    if (Object.keys(invalid).length > 0) {
      setValidationAttempt((value) => value + 1);
      return;
    }
    const content = draft;
    void run({
      kind: 'save',
      scope: `save:${basis?.id ?? 'new'}`,
      execute: (options) =>
        basis === undefined
          ? client.createProfile(content, options)
          : client.saveProfile(basis.id, { ...content, draftVersion: basis.draftVersion }, options),
    });
  };

  const retry = async () => {
    if (pending !== undefined || readAbort.current !== undefined) return;
    if (transaction.current !== undefined) {
      await run(transaction.current);
      return;
    }
    if (failure?.conflict !== true || profile === undefined) return;
    const request = new AbortController();
    readAbort.current = request;
    setReadingCurrent(true);
    try {
      const latest = current ?? (await client.readProfile(profile.id, { signal: request.signal }));
      if (!mounted.current || latest.id !== profile.id) return;
      save(latest);
    } catch {
      if (mounted.current)
        setFailure({ ...failure, message: 'The current profile could not be loaded. Try again.' });
    } finally {
      readAbort.current = undefined;
      if (mounted.current) setReadingCurrent(false);
    }
  };

  const loadCurrent = async () => {
    if (profile === undefined || pending !== undefined || readAbort.current !== undefined) return;
    const request = new AbortController();
    readAbort.current = request;
    setReadingCurrent(true);
    try {
      const latest = current ?? (await client.readProfile(profile.id, { signal: request.signal }));
      if (!mounted.current || latest.id !== profile.id) return;
      setProfile(latest);
      setDraft(draftOf(latest));
      setCurrent(undefined);
      setErrors({});
      setFailure(undefined);
    } catch {
      if (mounted.current)
        setFailure({
          message: 'The current profile could not be loaded. Try again.',
          conflict: true,
          uncertain: false,
          fields: {},
        });
    } finally {
      readAbort.current = undefined;
      if (mounted.current) setReadingCurrent(false);
    }
  };

  const publish = () => {
    if (profile === undefined || locked || dirty || failure?.conflict) return;
    const basis = profile;
    void run({
      kind: 'publish',
      scope: `publish:${basis.id}`,
      execute: async (options) => {
        const revision = await client.publishProfile(
          basis.id,
          { draftVersion: basis.draftVersion },
          options,
        );
        if (revision.profileId !== basis.id) throw new Error('Profile identity mismatch');
        return {
          ...basis,
          latestRevision: {
            revision: revision.revision,
            digest: revision.digest,
            publishedAt: revision.publishedAt,
          },
        };
      },
    });
  };

  const visibility = (value: ProfileVisibility) => {
    if (
      profile === undefined ||
      locked ||
      dirty ||
      failure?.conflict ||
      (value === 'public' && profile.latestRevision === undefined)
    )
      return;
    const basis = profile;
    void run({
      kind: 'visibility',
      scope: `visibility:${basis.id}:${value}`,
      execute: (options) => client.setProfileVisibility(basis.id, { visibility: value }, options),
    });
  };

  const retire = () => {
    if (profile === undefined || locked || dirty || failure?.conflict) return;
    const basis = profile;
    void run({
      kind: 'retire',
      scope: `retire:${basis.id}`,
      execute: (options) => client.retireProfile(basis.id, options),
    });
  };

  return {
    profile,
    draft,
    dirty,
    loading,
    readingCurrent,
    readError,
    failure,
    pending,
    unresolved,
    locked,
    errors,
    validationAttempt,
    success,
    change,
    save: () => save(),
    publish,
    visibility,
    retire,
    retry,
    loadCurrent,
    reload: () => setReload((value) => value + 1),
  };
}
