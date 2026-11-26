import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import {
  serviceRequest,
  ServiceError,
  type Service,
  type Denied,
  type ServiceSession,
} from './client';
export function ServicePage({
  service,
  children,
  renderStatus,
  viewer,
}: {
  readonly service: Service;
  readonly children: (session: ServiceSession, denied: Denied, notice?: ReactNode) => ReactNode;
  readonly renderStatus?: (content: ReactNode) => ReactNode;
  /**
   * Who the shell is signed in as, when that is known. A change re-checks the
   * service session, and the page is withheld until the session is confirmed
   * for the new viewer: another tab signing in as someone else must never show
   * them the work this page retained, even while the service cannot answer.
   */
  readonly viewer?: string;
}) {
  const [state, setState] = useState<{
    session?: ServiceSession;
    error?: string;
    anonymous?: boolean;
    forbidden?: boolean;
    /** The known viewer the session was last confirmed for. */
    confirmedFor?: string;
    /** The viewer a refusal answered; a different viewer earns a fresh check. */
    refusedFor?: string;
  }>({});
  const [revision, setRevision] = useState(0);
  const name = service === 'image-manager' ? 'Image Manager' : 'Event Gateway';
  const signedOutOfGateway =
    service === 'event-gateway' && state.anonymous === true && state.refusedFor === viewer;
  const currentViewer = useRef(viewer);
  useLayoutEffect(() => {
    currentViewer.current = viewer;
  }, [viewer]);
  // Signed out and not permitted are different answers: offering sign-in for a
  // 403 only returns the same account to the same refusal.
  const denied = useCallback<Denied>(
    (status) => {
      setState((current) =>
        service === 'event-gateway' && current.anonymous
          ? current
          : status === 403
            ? { forbidden: true, refusedFor: currentViewer.current }
            : { anonymous: true, refusedFor: currentViewer.current },
      );
    },
    [service],
  );
  useEffect(() => {
    if (signedOutOfGateway) return;
    const controller = new AbortController();
    let requestSequence = 0;
    const check = () => {
      const sequence = ++requestSequence;
      void serviceRequest<ServiceSession>(service, '/auth/session', {
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(8000)]),
      })
        .then((session) => {
          if (!controller.signal.aborted && sequence === requestSequence)
            // A denied mutation or logout wins over an earlier session check for
            // the same viewer. Reauthentication requires the explicit sign-in
            // navigation; a different viewer is a different question.
            setState((current) =>
              refusedFor(current, viewer)
                ? current
                : { session, confirmedFor: viewer ?? current.confirmedFor },
            );
        })
        .catch((error: unknown) => {
          if (!controller.signal.aborted && sequence === requestSequence)
            setState((current) =>
              refusedFor(current, viewer)
                ? current
                : error instanceof ServiceError && error.status === 403
                  ? { forbidden: true, refusedFor: viewer }
                  : error instanceof ServiceError && error.status === 401
                    ? { anonymous: true, refusedFor: viewer }
                    : {
                        ...current,
                        error: error instanceof Error ? error.message : 'Service unavailable.',
                      },
            );
        });
    };
    check();
    const focused = () => {
      if (document.visibilityState === 'visible') check();
    };
    window.addEventListener('focus', focused);
    const interval = window.setInterval(focused, 45000);
    return () => {
      controller.abort();
      window.clearInterval(interval);
      window.removeEventListener('focus', focused);
    };
  }, [service, revision, signedOutOfGateway, viewer]);
  const withheld =
    state.session !== undefined && viewer !== undefined && state.confirmedFor !== viewer;
  const notice = state.error ? (
    <Alert
      tone="error"
      actions={
        <Button
          onClick={() => {
            setState((current) => ({ ...current, error: undefined }));
            setRevision((value) => value + 1);
          }}
        >
          Try again
        </Button>
      }
    >
      {state.error}
    </Alert>
  ) : undefined;
  const status = (
    <>
      {notice}
      {state.anonymous ? (
        <section className="service-panel">
          <p>Sign in with NyxID to open {name}.</p>
          <a
            className="service-link"
            href={
              service === 'event-gateway'
                ? '/auth/login?return_to=%2Fevent-gateway'
                : `/api/${service}/auth/login`
            }
          >
            Sign in with NyxID
          </a>
        </section>
      ) : state.forbidden ? (
        // The alert is the whole answer; a panel around it would draw a second edge.
        <section className="service-forbidden">
          <Alert
            tone="warning"
            title={`Your account doesn't have ${name} permissions.`}
            actions={
              <Button
                onClick={() => {
                  setState({});
                  setRevision((value) => value + 1);
                }}
              >
                Check again
              </Button>
            }
          />
        </section>
      ) : state.error ? null : (
        <p role="status">Opening {name}…</p>
      )}
    </>
  );
  const statusView = renderStatus ? (
    renderStatus(status)
  ) : (
    <main className="service-page">{status}</main>
  );
  if (state.session && !refused(state)) {
    const content = children(state.session, denied, notice);
    // The session's page stays mounted while it is withheld, so work that does
    // belong to the confirmed viewer is still there when it is shown again.
    return (
      <>
        {withheld ? statusView : null}
        <div className="service-page__session" hidden={withheld} inert={withheld || undefined}>
          <Fragment
            key={JSON.stringify([state.session.subject, [...state.session.permissions].sort()])}
          >
            {renderStatus ? (
              content
            ) : (
              <main className="service-page">
                {notice}
                {content}
              </main>
            )}
          </Fragment>
        </div>
      </>
    );
  }
  return statusView;
}

/** A refusal outlives an earlier session check that is still in flight. */
function refused(state: { readonly anonymous?: boolean; readonly forbidden?: boolean }) {
  return state.anonymous === true || state.forbidden === true;
}

/** A refusal outlives a session check for the viewer it answered. */
function refusedFor(
  state: {
    readonly anonymous?: boolean;
    readonly forbidden?: boolean;
    readonly refusedFor?: string;
  },
  viewer: string | undefined,
) {
  return refused(state) && state.refusedFor === viewer;
}
