import { useEffect, useRef, useState } from 'react';
import markMarkup from '../assets/brand/cma-mark-animated.svg?raw';
import { ApiError, readTriggerPreview, type TriggerPreview } from '../api/client';
import { BouncingText } from '../sessions/BouncingText';
import { IconBranchOutline16, IconCheckOutline16, IconWarningOutline16 } from '../ui/icons';
import './Trigger.css';

interface PublicTriggerPreviewProps {
  readonly publicId: string;
  readonly onSignIn: () => void;
}

interface PreviewFailure {
  readonly title: string;
  readonly detail: string;
}

function failureFor(cause: unknown): PreviewFailure {
  if (cause instanceof ApiError) {
    if (cause.status === 404) {
      return {
        title: 'Trigger not found',
        detail: 'This README link does not identify an available CMA Trigger.',
      };
    }
    if (cause.status === 429) {
      return {
        title: 'Preview limit reached',
        detail: 'Wait a moment, then reload this page.',
      };
    }
    return { title: 'Trigger unavailable', detail: cause.message };
  }
  return {
    title: 'Trigger unavailable',
    detail: cause instanceof Error ? cause.message : String(cause),
  };
}

function resources(preview: TriggerPreview): string {
  const cpu = preview.resource_default.cpu_millis / 1000;
  const memory = preview.resource_default.memory_mib;
  return `${cpu.toLocaleString('en', { maximumFractionDigits: 2 })} CPU, ${memory.toLocaleString('en')} MiB memory`;
}

function duration(seconds: number): string {
  if (seconds % 3600 === 0) {
    const hours = seconds / 3600;
    return `${String(hours)} ${hours === 1 ? 'hour' : 'hours'}`;
  }
  if (seconds % 60 === 0) {
    return `${String(seconds / 60)} minutes`;
  }
  return `${String(seconds)} seconds`;
}

export function PublicTriggerPreview({ publicId, onSignIn }: PublicTriggerPreviewProps) {
  const [preview, setPreview] = useState<TriggerPreview | null>(null);
  const [failure, setFailure] = useState<PreviewFailure | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    heading.current?.focus();
  }, [publicId]);

  useEffect(() => {
    const controller = new AbortController();
    void readTriggerPreview(publicId, controller.signal)
      .then(setPreview)
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) {
          setFailure(failureFor(cause));
        }
      });
    return () => {
      controller.abort();
    };
  }, [publicId]);

  return (
    <div className="trigger-public">
      <header className="trigger-public__brand" aria-label="CMA">
        <span
          className="trigger-public__mark"
          aria-hidden="true"
          dangerouslySetInnerHTML={{ __html: markMarkup }}
        />
        <span>CMA</span>
      </header>
      <main className="trigger-public__main">
        <div className="trigger-public__content">
          {failure !== null ? (
            <section className="trigger-public__failure" role="alert">
              <IconWarningOutline16 />
              <div>
                <h1 ref={heading} tabIndex={-1}>
                  {failure.title}
                </h1>
                <p>{failure.detail}</p>
              </div>
            </section>
          ) : preview === null ? (
            <section className="trigger-public__loading" aria-busy="true">
              <h1 ref={heading} tabIndex={-1}>
                CMA Trigger
              </h1>
              <BouncingText text="Reading this Trigger" />
              <div className="trigger-public__skeleton skeleton" />
              <div className="trigger-public__skeleton trigger-public__skeleton--short skeleton" />
            </section>
          ) : (
            <>
              <header className="trigger-public__intro">
                <p className="eyebrow">Published by {preview.publisher_display_identity}</p>
                <h1 ref={heading} tabIndex={-1}>
                  {preview.public_summary}
                </h1>
                <p className="trigger-public__repository">
                  <IconBranchOutline16 />
                  <span>
                    {preview.repository_owner}/{preview.repository_name}
                  </span>
                </p>
              </header>

              <section className="trigger-band" aria-labelledby="trigger-preview-access">
                <h2 id="trigger-preview-access">Session access</h2>
                <dl className="trigger-facts">
                  <div>
                    <dt>Permission ceiling</dt>
                    <dd>{preview.permission_ceiling}</dd>
                  </div>
                  <div>
                    <dt>GitHub</dt>
                    <dd>Contributor connection for a public repository</dd>
                  </div>
                  <div>
                    <dt>Forking</dt>
                    <dd>{preview.allow_forking ? 'Allowed by the upstream' : 'Unavailable'}</dd>
                  </div>
                </dl>
              </section>

              <section className="trigger-band" aria-labelledby="trigger-preview-runtime">
                <h2 id="trigger-preview-runtime">Runtime</h2>
                <dl className="trigger-facts">
                  <div>
                    <dt>Default resources</dt>
                    <dd>{resources(preview)}</dd>
                  </div>
                  <div>
                    <dt>Timeout</dt>
                    <dd>{duration(preview.timeout_default_seconds)}</dd>
                  </div>
                  <div>
                    <dt>Billing</dt>
                    <dd>
                      {preview.billing_mode === 'zero_cost'
                        ? 'No CMA capacity charge'
                        : preview.billing_mode === 'contributor'
                          ? 'Contributor billed'
                          : 'Sponsored policy'}
                    </dd>
                  </div>
                </dl>
              </section>

              <section className="trigger-band" aria-labelledby="trigger-preview-capabilities">
                <h2 id="trigger-preview-capabilities">Required capabilities</h2>
                <ul className="trigger-capabilities">
                  {preview.required_capabilities.map((capability) => (
                    <li key={capability}>
                      <IconCheckOutline16 />
                      <span>{capability.replaceAll('_', ' ')}</span>
                    </li>
                  ))}
                </ul>
              </section>

              <footer className="trigger-public__action">
                {preview.availability === 'retired' ? (
                  <div className="notice notice--quiet" role="status">
                    <IconWarningOutline16 />
                    <div>
                      <p className="notice__title">This Trigger is retired</p>
                      <p className="notice__detail">
                        Existing Launches remain available to their owners.
                      </p>
                    </div>
                  </div>
                ) : (
                  <button type="button" className="button button--primary" onClick={onSignIn}>
                    Sign in to review
                  </button>
                )}
              </footer>
            </>
          )}
        </div>
      </main>
    </div>
  );
}
