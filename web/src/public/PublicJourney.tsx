import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ApiError,
  repeatChoiceFrom,
  type ExistingLaunchAgent,
  type LaunchRepeatChoice,
  type TriggerPreview,
} from '@chronoai/cma-sdk';
import { useAuth } from '../auth/context';
import type { SignInOptions } from '../auth/types';
import lockup from '../assets/brand/cma-lockup-horizontal.svg';
import { routePath, type AppRoute, type AppRouter } from '../routing/routes';
import { Button } from '../ui/Button';
import { publicCopy } from '../vocabulary/needs';
import { refusalSentence } from '../vocabulary/errors';
import type { PublicClient } from './client';
import { admit } from './admission';
import { createIntent, forgetIntent, readIntent, type BrowserIntent } from './intent';
import { Landing } from './Landing';
import { Progress } from './Progress';
import { RepeatChoice } from './RepeatChoice';
import './Public.css';

export type PublicRoute = Extract<AppRoute, { kind: 'public_trigger' | 'launch' }>;

export function PublicJourney({
  client,
  route,
  onNavigate,
}: {
  readonly client: PublicClient;
  readonly route: PublicRoute;
  readonly onNavigate: AppRouter['navigate'];
}) {
  const auth = useAuth();
  return (
    <div className="public-frame">
      <div className="public-axis">
        <a className="public-brand" href="/" aria-label="CMA">
          <img src={lockup} alt="CMA" />
        </a>
        <main>
          {route.kind === 'public_trigger' ? (
            <Entry key={routePath(route)} client={client} route={route} onNavigate={onNavigate} />
          ) : auth.state.status === 'authenticated' ? (
            <Progress
              key={route.launchId}
              client={client}
              id={route.launchId}
              onNavigate={onNavigate}
            />
          ) : auth.state.status === 'anonymous' ? (
            <Button variant="primary" onClick={() => void auth.signIn(routePath(route))}>
              {publicCopy.signIn}
            </Button>
          ) : (
            <p role="status">{publicCopy.loading}</p>
          )}
        </main>
      </div>
    </div>
  );
}

function Entry({
  client,
  route,
  onNavigate,
}: {
  readonly client: PublicClient;
  readonly route: Extract<PublicRoute, { kind: 'public_trigger' }>;
  readonly onNavigate: AppRouter['navigate'];
}) {
  const { state, signIn } = useAuth();
  const [preview, setPreview] = useState<TriggerPreview>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [existing, setExisting] = useState<ExistingLaunchAgent>();
  const [intent, setIntent] = useState<BrowserIntent | undefined>(() =>
    readIntent(routePath(route)),
  );
  const [revision, setRevision] = useState(0);
  const owner = state.status === 'authenticated' ? state.user.id : undefined;
  const allowed =
    state.status === 'authenticated' && state.user.capabilities.includes('launch_triggers');
  const attempted = useRef(new Set<string>());
  useEffect(() => {
    document.title = preview === undefined ? 'CMA' : publicCopy.pageTitle(preview.title);
  }, [preview]);
  const submit = useCallback(
    async (selected: BrowserIntent) => {
      if (owner === undefined || !allowed) return;
      setBusy(true);
      setError(undefined);
      try {
        const result = await admit(client, selected, owner);
        forgetIntent(selected.id);
        onNavigate({ kind: 'launch', launchId: result.id }, { replace: true });
      } catch (cause) {
        let repeat: ExistingLaunchAgent | undefined;
        try {
          repeat = repeatChoiceFrom(cause);
        } catch {
          repeat = undefined;
        }
        if (repeat !== undefined) {
          forgetIntent(selected.id);
          setExisting(repeat);
        } else {
          setError(refusalSentence(cause));
          if (cause instanceof ApiError && !cause.isRetryable) forgetIntent(selected.id);
        }
      } finally {
        setBusy(false);
      }
    },
    [allowed, client, onNavigate, owner],
  );
  useEffect(() => {
    const controller = new AbortController();
    void client.requests.preview(route.publicId, { signal: controller.signal }).then(
      (value) => {
        if (!controller.signal.aborted) setPreview(value);
      },
      (cause: unknown) => {
        if (!controller.signal.aborted) setError(refusalSentence(cause));
      },
    );
    return () => controller.abort();
  }, [client, route.publicId, revision]);
  useEffect(() => {
    if (
      preview?.availability !== 'published' ||
      intent === undefined ||
      owner === undefined ||
      (intent.owner !== undefined && intent.owner !== owner) ||
      !allowed
    )
      return;
    let live = true;
    void Promise.resolve().then(() => {
      const slot = `${owner}:${intent.key}`;
      if (!live || attempted.current.has(slot)) return;
      attempted.current.add(slot);
      void submit(intent);
    });
    return () => {
      live = false;
    };
  }, [allowed, intent, owner, preview?.availability, submit]);
  const start = (options?: SignInOptions, choice?: LaunchRepeatChoice) => {
    if (busy || state.status === 'checking') return;
    if (state.status === 'authenticated' && !allowed) {
      setError(publicCopy.readOnly);
      return;
    }
    let selected: BrowserIntent;
    try {
      const saved = readIntent(routePath(route));
      selected =
        choice === undefined
          ? ((saved?.owner === undefined || saved.owner === owner ? saved : undefined) ??
            createIntent(route))
          : createIntent(route, choice);
    } catch {
      setError(publicCopy.storageUnavailable);
      return;
    }
    setIntent(selected);
    if (state.status === 'anonymous') {
      setBusy(true);
      void signIn(selected.path, options).finally(() => setBusy(false));
    } else {
      attempted.current.add(`${owner}:${selected.key}`);
      void submit(selected);
    }
  };
  return (
    <>
      {preview === undefined ? (
        error === undefined ? (
          <p role="status">{publicCopy.loading}</p>
        ) : (
          <h1>{publicCopy.pageUnavailable}</h1>
        )
      ) : (
        <Landing
          preview={preview}
          anonymous={state.status === 'anonymous'}
          busy={busy || state.status === 'checking'}
          onStart={() => start()}
          onRegister={() => start({ screen: 'register' })}
        />
      )}
      {error === undefined ? null : (
        <div className="public-notice">
          <p className="public-error" role="alert">
            {error}
          </p>
          <Button
            disabled={busy}
            onClick={() => {
              if (preview === undefined) {
                setError(undefined);
                setRevision((value) => value + 1);
              } else start();
            }}
          >
            {publicCopy.retry}
          </Button>
        </div>
      )}
      {existing === undefined ? null : (
        <RepeatChoice
          existing={existing}
          busy={busy}
          onChoose={(choice) => start(undefined, choice)}
          onClose={() => setExisting(undefined)}
        />
      )}
    </>
  );
}
