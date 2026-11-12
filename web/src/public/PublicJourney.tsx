import { Presence } from '../ui/Presence';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  repeatChoiceFrom,
  type ExistingLaunchAgent,
  type LaunchRepeatChoice,
  type TriggerPreview,
} from '@chronoai/cma-sdk';
import { useAuth } from '../auth/context';
import {
  checkCredentialRecoveryOwner,
  CredentialAccountError,
  credentialSetupMessage,
  needsSandboxCredentialSetup,
} from '../auth/credentialSetup';
import { SandboxCredentialSetup } from '../auth/SandboxCredentialSetup';
import type { SignInOptions } from '../auth/types';
import lockup from '../assets/brand/cma-lockup-horizontal.svg';
import { routePath, type AppRoute, type AppRouter } from '../routing/routes';
import { Button } from '../ui/Button';
import { publicCopy } from '../vocabulary/needs';
import { refusalSentence } from '../vocabulary/errors';
import type { PublicClient } from './client';
import { admit, isSettledQuotaRefusal, isUnsettledCommand } from './admission';
import { createIntent, forgetIntent, readIntent, writeIntent, type BrowserIntent } from './intent';
import { Landing } from './Landing';
import { Progress } from './Progress';
import { RepeatChoice } from './RepeatChoice';
import './Public.css';
import { BrandLoading } from '../landing/LandingCharacter';

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
              owner={auth.state.user.id}
              onNavigate={onNavigate}
            />
          ) : auth.state.status === 'anonymous' ? (
            <Button variant="primary" onClick={() => void auth.signIn(routePath(route))}>
              {publicCopy.signIn}
            </Button>
          ) : (
            <BrandLoading label={publicCopy.loading} />
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
  const [intent, setIntent] = useState<BrowserIntent | undefined>(() =>
    readIntent(routePath(route)),
  );
  const [error, setError] = useState<string | undefined>(
    intent?.credentialRecovery === undefined ? undefined : credentialSetupMessage,
  );
  const [busy, setBusy] = useState(false);
  const [automatic, setAutomatic] = useState(false);
  const entryRequest = useRef<ReturnType<PublicClient['requests']['entryIntent']> | undefined>(
    undefined,
  );
  const entryApplied = useRef(false);
  const signingIn = useRef(false);
  const [needsSignIn, setNeedsSignIn] = useState(intent?.credentialRecovery !== undefined);
  const [existing, setExisting] = useState<ExistingLaunchAgent>();
  const [revision, setRevision] = useState(0);
  const owner = state.status === 'authenticated' ? state.user.id : undefined;
  const credentialReturnTo = routePath({ kind: 'public_trigger', publicId: route.publicId });
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
        const subject = selected.owner ?? owner;
        await checkCredentialRecoveryOwner(
          selected.credentialRecovery === undefined ? undefined : subject,
        );
        const result = await admit(client, selected, subject);
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
          const refused = needsSandboxCredentialSetup(cause);
          setNeedsSignIn(refused || cause instanceof CredentialAccountError);
          if (refused) {
            const retained: BrowserIntent = {
              ...selected,
              owner: selected.owner ?? owner,
              credentialRecovery: 'refused',
            };
            setIntent(retained);
            try {
              writeIntent(retained);
            } catch {
              setError(publicCopy.storageUnavailable);
            }
          } else if (!isUnsettledCommand(cause)) {
            forgetIntent(selected.id);
            if (isSettledQuotaRefusal(cause)) setIntent(undefined);
          }
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
      preview.always_review !== false ||
      intent !== undefined
    )
      return;
    let live = true;
    entryRequest.current ??= client.requests.entryIntent(route.publicId);
    void entryRequest.current.then(
      (entry) => {
        if (
          !live ||
          entryApplied.current ||
          !entry.accepted ||
          entry.placement_id !== route.placementId
        )
          return;
        entryApplied.current = true;
        try {
          const selected = createIntent(route);
          setAutomatic(true);
          setIntent(selected);
        } catch {
          setError(publicCopy.storageUnavailable);
        }
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client, intent, preview?.always_review, preview?.availability, route]);
  useEffect(() => {
    if (!automatic || intent === undefined || state.status !== 'anonymous' || signingIn.current)
      return;
    signingIn.current = true;
    void signIn(intent.path);
  }, [automatic, intent, signIn, state.status]);
  useEffect(() => {
    if (
      preview?.availability !== 'published' ||
      intent === undefined ||
      intent.credentialRecovery !== undefined ||
      owner === undefined ||
      (intent.owner !== undefined && intent.owner !== owner) ||
      !allowed ||
      needsSignIn
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
  }, [allowed, intent, needsSignIn, owner, preview?.availability, submit]);
  const start = (options?: SignInOptions, choice?: LaunchRepeatChoice) => {
    if (busy || needsSignIn || state.status === 'checking') return;
    if (state.status === 'authenticated' && !allowed) {
      setError(publicCopy.readOnly);
      return;
    }
    let selected: BrowserIntent;
    try {
      const saved =
        intent?.credentialRecovery === undefined ? readIntent(routePath(route)) : intent;
      selected =
        saved?.credentialRecovery !== undefined
          ? saved.credentialRecovery === 'refused'
            ? { ...saved, key: crypto.randomUUID(), credentialRecovery: 'retry' }
            : saved
          : choice === undefined
            ? ((saved?.owner === undefined || saved.owner === owner ? saved : undefined) ??
              createIntent(route))
            : createIntent(route, choice);
      writeIntent(selected);
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
          <BrandLoading label={publicCopy.loading} />
        ) : (
          <h1>{publicCopy.pageUnavailable}</h1>
        )
      ) : automatic && error === undefined ? (
        <>
          <header className="public-heading">
            <h1>{preview.title}</h1>
          </header>
          <BrandLoading
            label={
              state.status === 'anonymous' || state.status === 'checking'
                ? 'Signing you in'
                : 'Preparing your agent'
            }
          />
        </>
      ) : (
        <Landing
          preview={preview}
          anonymous={state.status === 'anonymous'}
          busy={busy || needsSignIn || state.status === 'checking'}
          onStart={() => start()}
          onRegister={() => start({ screen: 'register' })}
        />
      )}
      <Presence>
        {error === undefined ? null : (
          <div className="public-notice">
            <p className="public-error" role="alert">
              {error}
            </p>
            {needsSignIn ? (
              <SandboxCredentialSetup
                returnTo={credentialReturnTo}
                expectedSubject={intent?.owner}
                onChecked={() => {
                  setNeedsSignIn(false);
                  setError(undefined);
                }}
              />
            ) : (
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
            )}
          </div>
        )}
      </Presence>
      <Presence>
        {existing === undefined ? null : (
          <RepeatChoice
            existing={existing}
            busy={busy}
            onChoose={(choice) => start(undefined, choice)}
            onClose={() => setExisting(undefined)}
          />
        )}
      </Presence>
    </>
  );
}
