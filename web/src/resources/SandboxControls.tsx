import { useState } from 'react';
import type { EffectiveAccess, SandboxStatus } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import type { useSandboxControl } from './useSandboxControl';
import { controlCapability } from './useSandboxControl';

type Control = ReturnType<typeof useSandboxControl>;
const failures = {
  authority_revoked: 'Access changed before this request could be scheduled.',
  queue_expired: 'This request expired before it could be scheduled.',
  target_changed: 'The Sandbox changed before this request could be scheduled.',
  not_eligible: 'The Sandbox cannot perform this action in its current state.',
  connections_required: 'The Sandbox owner needs to reconnect its required services.',
  credential_setup_required: 'The Sandbox owner needs to finish credential setup.',
};
export function SandboxControls({
  control,
  access,
  status,
  revision,
}: {
  readonly control: Control;
  readonly access: EffectiveAccess;
  readonly status: SandboxStatus;
  readonly revision?: number;
}) {
  const [confirm, setConfirm] = useState(false);
  const { intent, busy } = control;
  const canManage = access.capabilities.includes('manage_sandbox');
  const canRetire = access.capabilities.includes('manage_sharing');
  if (!canRetire && confirm) setConfirm(false);
  if (intent) {
    const receipt = intent.receipt;
    const terminal = intent.refused || (receipt && receipt.state.status !== 'queued');
    return (
      <div className="sandbox-controls" aria-label="Sandbox controls">
        <p role="status">
          {busy
            ? 'Submitting Sandbox request…'
            : receipt?.state.status === 'queued'
              ? 'Request queued. Checking progress…'
              : receipt?.state.status === 'applied'
                ? 'Request applied. Sandbox status shows runtime progress.'
                : receipt?.state.status === 'rejected'
                  ? failures[receipt.state.reason]
                  : intent.message}
        </p>
        {receipt && intent.message && <p>{intent.message}</p>}
        {terminal ? (
          <Button onClick={control.dismiss}>Done</Button>
        ) : (
          !receipt && (
            <Button
              disabled={
                busy || !access.capabilities.includes(controlCapability(intent.body.command))
              }
              onClick={control.retry}
            >
              Retry Sandbox request
            </Button>
          )
        )}
      </div>
    );
  }
  if (!canManage && !canRetire) return null;
  if (revision === undefined)
    return <p>Sandbox controls are unavailable until a current revision is available.</p>;
  const canPause = canManage && status === 'active';
  const canRevive = canManage && status === 'paused';
  const canRemove = canRetire && status !== 'retired' && status !== 'retiring';
  return (
    <div className="sandbox-controls" aria-label="Sandbox controls">
      <div className="sandbox-controls__actions">
        {canPause && <Button onClick={() => control.start('pause')}>Pause Sandbox</Button>}
        {canRevive && <Button onClick={() => control.start('revive')}>Resume Sandbox</Button>}
        {canRemove && !confirm && (
          <Button variant="danger" onClick={() => setConfirm(true)}>
            Retire Sandbox
          </Button>
        )}
      </div>
      {canRemove && confirm && (
        <div>
          <p>Retire this Sandbox and stop its Agents? It cannot be resumed after retirement.</p>
          <div className="sandbox-controls__actions">
            <Button
              variant="danger"
              onClick={() => {
                setConfirm(false);
                control.start('retire');
              }}
            >
              Confirm retirement
            </Button>
            <Button onClick={() => setConfirm(false)}>Cancel</Button>
          </div>
        </div>
      )}
    </div>
  );
}
