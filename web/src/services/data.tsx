import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
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
  busy,
  error,
  retry,
}: {
  readonly busy: boolean;
  readonly error?: string;
  readonly retry: () => void;
}) {
  return (
    <>
      {busy ? <p role="status">Loading…</p> : null}
      {error ? (
        <div role="alert">
          <p>{error}</p>
          <Button onClick={retry}>Try again</Button>
        </div>
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
  accept?: string;
  maxBytes?: number;
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
            body: await formBody(event.currentTarget, fields),
          };
          setPending(request);
          await send(request);
        } catch (cause) {
          setError(
            cause instanceof Error ? cause.message : 'The file could not be read. Try again.',
          );
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
              accept={field.accept}
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

async function formBody(form: HTMLFormElement, fields: readonly Field[]): Promise<string> {
  const entries: [string, string][] = [];
  for (const [name, value] of new FormData(form).entries()) {
    if (value instanceof File) {
      if (value.name === '' && value.size === 0) continue;
      const field = fields.find((field) => field.name === name);
      const limit = field?.maxBytes ?? 16 * 1024;
      if (value.size === 0 || value.size > limit)
        throw new Error(`${field?.label ?? 'File'} must contain 1–${String(limit)} bytes.`);
      const content = await readTextFile(value);
      if (content.includes('\0')) throw new Error('The file must contain plain UTF-8 text.');
      entries.push([name, content]);
    } else {
      entries.push([name, value]);
    }
  }
  return JSON.stringify(Object.fromEntries(entries));
}

function readTextFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error('The file could not be read. Choose it again.'));
    reader.onabort = reader.onerror;
    reader.onload = () => {
      try {
        resolve(new TextDecoder('utf-8', { fatal: true }).decode(reader.result as ArrayBuffer));
      } catch {
        reject(new Error('The file must contain plain UTF-8 text.'));
      }
    };
    reader.readAsArrayBuffer(file);
  });
}
