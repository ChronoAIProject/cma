import { useRef, useState } from 'react';
import type { SandboxDetail, SandboxRuntimeUpdate } from '../api/types';
import { Button } from '../ui/Button';
import { IconCopyOutline16 } from '../ui/icons';
import type { StateDotState } from '../ui/StateDot';

export function runtimeLifecycleNotice(
  status: SandboxDetail['status'] | undefined,
  update: SandboxRuntimeUpdate,
  stale: boolean,
): { title: string; detail: string; tone: StateDotState } | undefined {
  if (status === undefined) return;
  if (status === 'retired')
    return {
      title: 'Environment retired',
      detail: 'This environment is no longer running. Runtime details are retained for reference.',
      tone: 'neutral',
    };
  if (status === 'retiring')
    return {
      title: 'Environment retiring',
      detail: 'Retirement is in progress. Runtime details reflect the last reported state.',
      tone: 'neutral',
    };
  if (stale)
    return {
      title: 'Runtime status unavailable',
      detail:
        'The latest runtime status could not be confirmed. Last reported details are shown below.',
      tone: 'warning',
    };
  if (update.state === 'up_to_date' && update.currentVersionConfirmed === false)
    return {
      title: 'Runtime version unconfirmed',
      detail: 'The current Desktop runtime could not be confirmed.',
      tone: 'neutral',
    };
  if (
    update.state === 'up_to_date' &&
    update.currentVersionConfirmed === true &&
    update.currentVersion !== update.latestVersion
  )
    return {
      title: 'Desktop runtime update unavailable',
      detail: 'The current version is shown below.',
      tone: 'neutral',
    };
  if (update.state === 'up_to_date' && !update.preservesHome)
    return {
      title: 'Managed runtime updates unavailable',
      detail: 'This Sandbox is not eligible for automatic CMA runtime updates.',
      tone: 'neutral',
    };
  if (
    update.state === 'up_to_date' &&
    (status !== 'active' ||
      !update.currentVersion ||
      update.currentVersion !== update.latestVersion)
  )
    return {
      title: 'Runtime version unconfirmed',
      detail:
        'A current runtime on an active environment has not been confirmed. Retained version details do not establish that it is up to date.',
      tone: 'neutral',
    };
}

export function SandboxRuntimeFacts({
  sandboxId,
  runtimeVersion,
  update,
  target = update?.targetVersion,
  copyable = false,
  sandboxStatus,
}: {
  readonly sandboxId?: string;
  readonly runtimeVersion?: string;
  readonly update?: SandboxRuntimeUpdate;
  readonly target?: string;
  readonly copyable?: boolean;
  readonly sandboxStatus?: SandboxDetail['status'];
}) {
  const current = update === undefined ? runtimeVersion : update.currentVersion;
  const rows = [
    ...(sandboxId === undefined ? [] : [['Sandbox ID', sandboxId]]),
    ['Current runtime', current || 'Not reported'],
    ...(update?.imageOrigin === 'managed_desktop' ? [['Runtime channel', 'Managed Agent']] : []),
    ...(update?.imageOrigin === 'managed_machine' ? [['Runtime channel', 'Managed Machine']] : []),
    ...(update === undefined
      ? []
      : [
          [
            update.preservesHome ? 'Latest runtime' : 'Latest managed runtime',
            update.latestVersion,
          ],
        ]),
    ...(target && target !== update?.latestVersion ? [['Requested runtime', target]] : []),
    ...(update === undefined
      ? []
      : [
          [
            'Automatic updates',
            !update.preservesHome
              ? 'Not available for this Sandbox'
              : sandboxStatus === 'failed' && !update.canUpdate
                ? 'Unavailable while the Sandbox needs attention'
                : `Daily at ${update.schedule.localTime} SGT (${update.schedule.timezone}), when idle`,
          ],
        ]),
  ];
  const text = rows.map(([name, value]) => `${name}: ${value}`).join('\n');
  return (
    <div className="sandbox-runtime-facts">
      <dl>
        {rows.map(([name, value]) => (
          <div key={name}>
            <dt>{name}</dt>
            <dd>
              {name === 'Automatic updates' ||
              name === 'Runtime channel' ||
              value === 'Not reported' ? (
                value
              ) : (
                <code>{value}</code>
              )}
            </dd>
          </div>
        ))}
      </dl>
      {copyable ? <CopyFacts key={text} text={text} rows={rows.length * 2} /> : null}
    </div>
  );
}

function selectDetails(element: HTMLTextAreaElement | null) {
  element?.focus();
  element?.select();
}

function CopyFacts({ text, rows }: { readonly text: string; readonly rows: number }) {
  const [feedback, setFeedback] = useState<{ state: 'copied' | 'failed'; attempt: number }>();
  const attempt = useRef(0);
  return (
    <div className="sandbox-runtime-facts__copy">
      <Button
        icon={<IconCopyOutline16 />}
        onClick={() => {
          const id = ++attempt.current;
          setFeedback(undefined);
          void Promise.resolve()
            .then(() => navigator.clipboard.writeText(text))
            .then(
              () => {
                if (id === attempt.current) setFeedback({ state: 'copied', attempt: id });
              },
              () => {
                if (id === attempt.current) setFeedback({ state: 'failed', attempt: id });
              },
            );
        }}
      >
        Copy technical details
      </Button>
      {feedback ? (
        <p role="status">
          {feedback.state === 'copied'
            ? 'Technical details copied.'
            : 'Copy unavailable. Select and copy the details below.'}
        </p>
      ) : null}
      {feedback?.state === 'failed' ? (
        <textarea
          key={feedback.attempt}
          ref={selectDetails}
          aria-label="Technical details to copy"
          readOnly
          value={text}
          rows={rows}
          onFocus={(event) => event.currentTarget.select()}
        />
      ) : null}
    </div>
  );
}
