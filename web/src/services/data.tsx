import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import { Input } from '../ui/Input';
import { ServiceError, serviceRequest, type Denied, type Service } from './client';
export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}
export function usePage<T>(service: Service, path: string, denied: Denied) {
  const [value, setValue] = useState<Page<T>>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(true);
  const [revision, setRevision] = useState(0);
  const [cursor, setCursor] = useState('');
  const refresh = useCallback(() => {
    setBusy(true);
    setCursor('');
    setRevision((v) => v + 1);
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    serviceRequest<Page<T>>(
      service,
      `${path}${cursor ? `${path.includes('?') ? '&' : '?'}after=${encodeURIComponent(cursor)}` : ''}`,
      { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(15000)]) },
    )
      .then((next) => {
        if (!controller.signal.aborted) {
          setValue((previous) =>
            cursor ? { ...next, items: [...(previous?.items ?? []), ...next.items] } : next,
          );
          setError(undefined);
        }
      })
      .catch((e: unknown) => {
        if (!controller.signal.aborted) {
          if (e instanceof ServiceError && [401, 403].includes(e.status)) {
            setValue(undefined);
            if ([401, 403].includes(e.status)) denied(e.status);
          }
          setError(e instanceof Error ? e.message : 'Service unavailable.');
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setBusy(false);
      });
    return () => controller.abort();
  }, [service, path, cursor, revision, denied]);
  return {
    value,
    error,
    busy,
    refresh,
    firstPage: cursor === '',
    more: () => {
      if (value?.next_cursor) {
        setBusy(true);
        setCursor(value.next_cursor);
      }
    },
  };
}
export function PageStatus({
  busy = false,
  error,
  retry,
}: {
  readonly busy?: boolean;
  readonly error?: string;
  readonly retry: () => void;
}) {
  return (
    <>
      {busy ? <SkeletonRows label="Loading" /> : null}
      {error ? (
        <Alert tone="error" actions={<Button onClick={retry}>Try again</Button>}>
          {error}
        </Alert>
      ) : null}
    </>
  );
}
export interface Field {
  name: string;
  label: string;
  initial?: string;
  required?: boolean;
  pattern?: string;
  maxLength?: number;
  type?: string;
  control?: 'input' | 'textarea';
}
export function WriteForm({
  service,
  path,
  label,
  fields,
  onDone,
  denied,
  method = 'POST',
  revision,
}: {
  readonly service: Service;
  readonly path: string;
  readonly label: string;
  readonly fields: readonly Field[];
  readonly onDone: () => void;
  readonly denied: Denied;
  readonly method?: string;
  readonly revision?: number;
}) {
  const [pending, setPending] = useState<{ key: string; body: string }>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const submitting = useRef(false);
  async function send(request: { key: string; body: string }) {
    try {
      await serviceRequest(service, path, {
        method,
        body: request.body,
        headers: {
          'Idempotency-Key': request.key,
          ...(revision === undefined ? {} : { 'If-Match': String(revision) }),
        },
      });
      setPending(undefined);
      onDone();
    } catch (e) {
      if (e instanceof ServiceError && e.status >= 400 && e.status < 500) {
        setPending(undefined);
        if ([401, 403].includes(e.status)) denied(e.status);
      }
      setError(e instanceof Error ? e.message : 'The outcome is unknown. Retry this request.');
    }
  }
  return (
    <form
      className="service-form"
      onSubmit={async (event) => {
        event.preventDefault();
        if (submitting.current) return;
        submitting.current = true;
        setBusy(true);
        setError(undefined);
        try {
          const request = pending ?? {
            key: crypto.randomUUID(),
            body: JSON.stringify(Object.fromEntries(new FormData(event.currentTarget))),
          };
          setPending(request);
          await send(request);
        } finally {
          submitting.current = false;
          setBusy(false);
        }
      }}
    >
      {fields.map((field) => (
        <label key={field.name}>
          {field.label}
          {field.control === 'textarea' ? (
            <textarea
              name={field.name}
              defaultValue={field.initial ?? ''}
              required={field.required}
              maxLength={field.maxLength}
              disabled={busy || pending !== undefined}
            />
          ) : (
            <Input
              name={field.name}
              type={field.type ?? 'text'}
              defaultValue={field.initial ?? ''}
              required={field.required}
              pattern={field.pattern}
              maxLength={field.maxLength}
              disabled={busy || pending !== undefined}
            />
          )}
        </label>
      ))}
      {error ? (
        <p role="alert">
          {error}
          {pending ? ' The outcome is unconfirmed. Retry sends the same request.' : ''}
        </p>
      ) : null}
      <div>
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? 'Submitting…' : pending ? 'Retry same request' : label}
        </Button>
      </div>
    </form>
  );
}
