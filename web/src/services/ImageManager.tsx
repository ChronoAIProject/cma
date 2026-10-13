import { useEffect, useState } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { can, segment, serviceRequest, ServiceError, type ServiceSession } from './client';
import { PageStatus, usePage, WriteForm } from './data';
import { imageVisibilityLabel } from '../create/imageCatalog';
interface Image {
  id: string;
  owner: string;
  name: string;
  description: string;
  shares: string[];
  visibility?: 'private' | 'shared' | 'public';
  retired: boolean;
  revision: number;
}
interface Build {
  id: string;
  phase: string;
  error?: string | null;
  reference: string | null;
  created_at_ms: number;
  source?: { repository: string; commit: string };
}
export function ImageManager({
  session,
  denied,
}: {
  readonly session: ServiceSession;
  readonly denied: () => void;
}) {
  const [scope, setScope] = useState('all');
  return (
    <>
      <label className="service-field">
        Image catalog
        <select value={scope} onChange={(event) => setScope(event.target.value)}>
          <option value="all">All accessible images</option>
          <option value="owned">Your images</option>
          <option value="private">Your private images</option>
          <option value="public">Public images</option>
          <option value="shared">Shared with you</option>
        </select>
      </label>
      <ImageCatalog key={scope} scope={scope} session={session} denied={denied} />
    </>
  );
}
function ImageCatalog({
  scope,
  session,
  denied,
}: {
  readonly scope: string;
  readonly session: ServiceSession;
  readonly denied: () => void;
}) {
  const catalog = usePage<Image>('image-manager', `/v1/images?scope=${scope}`, denied);
  const [selected, setSelected] = useState<string>();
  const image = catalog.value?.items.find((i) => i.id === selected);
  return (
    <>
      <div className="service-actions">
        <h2>Images</h2>
        <Button onClick={catalog.refresh} disabled={catalog.busy}>
          Refresh
        </Button>
      </div>
      {can(session, 'image-manager', 'images:create') ? (
        <details className="service-panel">
          <summary>Create an image</summary>
          <WriteForm
            service="image-manager"
            path="/v1/images"
            label="Create image"
            fields={[
              { name: 'name', label: 'Image name', required: true, maxLength: 128 },
              { name: 'description', label: 'Description', maxLength: 4096 },
            ]}
            denied={denied}
            onDone={catalog.refresh}
          />
        </details>
      ) : null}
      <PageStatus busy={catalog.busy} error={catalog.error} retry={catalog.refresh} />
      {catalog.value?.items.length === 0 ? <p>No images in this catalog.</p> : null}
      <ul className="service-list">
        {catalog.value?.items.map((i) => (
          <li key={i.id}>
            <div className="service-actions">
              <div>
                <h3>
                  {i.name}
                  {i.retired ? ' · Retired' : ''}
                </h3>
                <p>
                  {i.description || 'No description'}
                  {' · '}
                  {imageVisibilityLabel(i.visibility)}
                </p>
              </div>
              <Button onClick={() => setSelected(i.id)} aria-expanded={selected === i.id}>
                Open image
              </Button>
            </div>
          </li>
        ))}
      </ul>
      {catalog.value?.next_cursor ? (
        <Button disabled={catalog.busy} onClick={catalog.more}>
          Load more images
        </Button>
      ) : null}
      {image ? (
        <ImageDetails
          key={image.id}
          image={image}
          session={session}
          denied={denied}
          changed={catalog.refresh}
        />
      ) : null}
    </>
  );
}
function ImageDetails({
  image,
  session,
  denied,
  changed,
}: {
  readonly image: Image;
  readonly session: ServiceSession;
  readonly denied: () => void;
  readonly changed: () => void;
}) {
  const owner = image.owner === session.subject;
  const base = `/v1/images/${segment(image.id)}`;
  const builds = usePage<Build>(
    'image-manager',
    `${base}/${owner && can(session, 'image-manager', 'builds:read') ? 'builds' : 'versions'}`,
    denied,
  );
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [subject, setSubject] = useState('');
  const [retiring, setRetiring] = useState(false);
  const [logs, setLogs] = useState<string>();
  const { refresh } = builds;
  const pending =
    builds.value?.items.some((b) => ['queued', 'dispatching', 'running'].includes(b.phase)) ??
    false;
  useEffect(() => {
    if (!pending || !builds.firstPage) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') refresh();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [pending, refresh, builds.firstPage]);
  async function mutate(path: string, method: string, body?: unknown) {
    setBusy(true);
    setError(undefined);
    try {
      await serviceRequest('image-manager', path, {
        method,
        headers: { 'If-Match': String(image.revision) },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      setRetiring(false);
      changed();
    } catch (e) {
      if (e instanceof ServiceError && [401, 403].includes(e.status)) denied();
      setError(
        e instanceof Error ? e.message : 'Request failed. Refresh the image before trying again.',
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="service-panel" aria-label={`${image.name} details`}>
      <h2>{image.name}</h2>
      {owner && !image.retired && can(session, 'image-manager', 'builds:create') ? (
        <details>
          <summary>Build a new version</summary>
          <p>
            Choose a public GitHub repository and an exact commit. The build publishes to your CMA
            image registry.
          </p>
          <WriteForm
            service="image-manager"
            path={`${base}/builds`}
            label="Start build"
            denied={denied}
            onDone={() => {
              refresh();
              changed();
            }}
            fields={[
              { name: 'repository', label: 'GitHub repository URL', required: true, type: 'url' },
              {
                name: 'commit',
                label: 'Commit SHA',
                required: true,
                pattern: '[0-9a-f]{40}',
                maxLength: 40,
              },
              {
                name: 'dockerfile',
                label: 'Dockerfile path',
                initial: 'Dockerfile',
                required: true,
              },
              { name: 'context', label: 'Build context directory', initial: '.', required: true },
              {
                name: 'dockerfile_content',
                label: 'Import Dockerfile (optional)',
                type: 'file',
                accept: '.dockerfile,Dockerfile,text/plain',
                maxBytes: 16 * 1024,
              },
            ]}
          />
        </details>
      ) : null}
      <div className="service-actions">
        <h3>{owner ? 'Builds and versions' : 'Versions'}</h3>
        <Button onClick={refresh} disabled={builds.busy}>
          Refresh builds
        </Button>
      </div>
      <PageStatus busy={builds.busy} error={builds.error} retry={refresh} />
      {builds.value?.items.length === 0 ? <p>No builds available.</p> : null}
      <ul className="service-list">
        {builds.value?.items.map((b) => (
          <li key={b.id}>
            <p>
              {b.phase} · {new Date(b.created_at_ms).toLocaleString()}
            </p>
            {b.source ? (
              <p>
                {b.source.repository} · <code>{b.source.commit}</code>
              </p>
            ) : null}
            {b.error === 'submission_outcome_unconfirmed' ? (
              <p>
                Waiting for the build service to confirm this request. It will not be submitted
                twice.
              </p>
            ) : null}
            {b.error === 'submission_rejected' ? (
              <p role="alert">
                The build service refused this request. Check the build configuration and service
                permissions before starting a new build.
              </p>
            ) : null}
            {b.reference ? <code>{b.reference}</code> : null}
            {owner && can(session, 'image-manager', 'builds:read') ? (
              <div>
                <Button
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    void serviceRequest<unknown>(
                      'image-manager',
                      `/v1/builds/${segment(b.id)}/logs`,
                    )
                      .then((value) => setLogs(JSON.stringify(value, null, 2)))
                      .catch((e: unknown) => {
                        if (e instanceof ServiceError && [401, 403].includes(e.status)) denied();
                        setError(e instanceof Error ? e.message : 'Logs unavailable.');
                      })
                      .finally(() => setBusy(false));
                  }}
                >
                  View build logs
                </Button>
              </div>
            ) : null}
          </li>
        ))}
      </ul>
      {builds.value?.next_cursor ? (
        <Button disabled={builds.busy} onClick={builds.more}>
          Load more builds
        </Button>
      ) : null}
      {logs ? (
        <details open>
          <summary>Build logs</summary>
          <pre>{logs}</pre>
        </details>
      ) : null}
      {owner && can(session, 'image-manager', 'images:share') ? (
        <details>
          <summary>Sharing</summary>
          <p>
            {image.visibility === 'public'
              ? 'Public: signed-in Image Manager users can discover this image and use its successful versions.'
              : 'Private: only you and the accounts listed below can use this image.'}
          </p>
          {image.visibility === 'public' ? (
            <Button
              disabled={busy}
              onClick={() => void mutate(`${base}/visibility`, 'PUT', { visibility: 'private' })}
            >
              Make private
            </Button>
          ) : !image.retired ? (
            <Button
              disabled={busy}
              onClick={() => void mutate(`${base}/visibility`, 'PUT', { visibility: 'public' })}
            >
              Make public
            </Button>
          ) : null}
          <p>
            Shared accounts can view and use successful versions. They cannot change or build this
            image. Making it private preserves these individual shares and existing Agents.
          </p>
          <form
            className="service-form"
            onSubmit={(e) => {
              e.preventDefault();
              void mutate(`${base}/shares/${segment(subject)}`, 'PUT');
            }}
          >
            <label>
              NyxID account ID
              <Input
                value={subject}
                onChange={(e) => setSubject(e.target.value)}
                required
                maxLength={512}
              />
            </label>
            <div>
              <Button type="submit" disabled={busy || !subject.trim()}>
                Share image
              </Button>
            </div>
          </form>
          <ul className="service-list">
            {image.shares.map((account) => (
              <li key={account}>
                {account}{' '}
                <Button
                  disabled={busy}
                  onClick={() => void mutate(`${base}/shares/${segment(account)}`, 'DELETE')}
                >
                  Remove access
                </Button>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      {owner && !image.retired && can(session, 'image-manager', 'images:delete') ? (
        <div>
          {retiring ? (
            <>
              <p>
                Retire {image.name}? This prevents new builds and new use. Existing Agents keep
                their image.
              </p>
              <Button variant="danger" disabled={busy} onClick={() => void mutate(base, 'DELETE')}>
                Confirm retirement
              </Button>
              <Button disabled={busy} onClick={() => setRetiring(false)}>
                Cancel
              </Button>
            </>
          ) : (
            <Button onClick={() => setRetiring(true)}>Retire image</Button>
          )}
        </div>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
