import { Fragment, useCallback, useEffect, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { serviceRequest, ServiceError, type Service, type ServiceSession } from './client';
export function ServicePage({
  service,
  children,
  renderStatus,
}: {
  readonly service: Service;
  readonly children: (session: ServiceSession, denied: () => void, notice?: ReactNode) => ReactNode;
  readonly renderStatus?: (content: ReactNode) => ReactNode;
}) {
  const [state, setState] = useState<{
    session?: ServiceSession;
    error?: string;
    anonymous?: boolean;
  }>({});
  const [revision, setRevision] = useState(0);
  const name = service === 'image-manager' ? 'Image Manager' : 'Event Gateway';
  const denied = useCallback(() => {
    setState({ anonymous: true });
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    let requestSequence = 0;
    const check = () => {
      const sequence = ++requestSequence;
      void serviceRequest<ServiceSession>(service, '/auth/session', {
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(8000)]),
      })
        .then((session) => {
          if (!controller.signal.aborted && sequence === requestSequence)
            // A denied mutation or logout wins over an earlier session check.
            // Reauthentication requires the explicit sign-in navigation.
            setState((current) => (current.anonymous ? current : { session }));
        })
        .catch((error: unknown) => {
          if (!controller.signal.aborted && sequence === requestSequence)
            setState((current) =>
              current.anonymous
                ? current
                : error instanceof ServiceError && [401, 403].includes(error.status)
                  ? { anonymous: true }
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
  }, [service, revision]);
  const notice = state.error ? (
    <section className="service-panel" role="alert">
      <p>{state.error}</p>
      <Button
        onClick={() => {
          setState((current) => ({ ...current, error: undefined }));
          setRevision((value) => value + 1);
        }}
      >
        Try again
      </Button>
    </section>
  ) : undefined;
  if (state.session && !state.anonymous) {
    const content = children(state.session, denied, notice);
    return (
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
    );
  }
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
      ) : state.error ? null : (
        <p role="status">Opening {name}…</p>
      )}
    </>
  );
  return renderStatus ? renderStatus(status) : <main className="service-page">{status}</main>;
}
