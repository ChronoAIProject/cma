import type { ChannelCreationState } from './channelLifecycle';

export function ChannelLifecycleStatus({
  state,
  addressAvailable,
  stale,
  compact = false,
}: {
  readonly state: ChannelCreationState;
  readonly addressAvailable: boolean;
  readonly stale: boolean;
  readonly compact?: boolean;
}) {
  const label =
    state.kind === 'complete'
      ? 'Setup complete'
      : state.kind === 'incomplete'
        ? 'Creation incomplete'
        : state.kind === 'unconfirmed'
          ? 'Creation status unconfirmed'
          : 'Legacy record';
  if (compact) {
    if (state.kind === 'legacy' && !stale) return null;
    return (
      <span className="page-header__chip">
        {label}
        {stale ? ' — status stale' : ''}
      </span>
    );
  }
  return (
    <div role="status" className="channel-field-hint">
      <p>
        {label}
        {stale ? ' — status stale; refresh required' : ''}
        {!addressAvailable ? ' — channel address unavailable' : ''}.
      </p>
      {stale || state.kind === 'incomplete' || state.kind === 'unconfirmed' ? (
        <p>
          Refresh to confirm this channel’s current setup. Editing, adding producers, sending and
          testing are paused. Your draft and any unconfirmed request are retained.
        </p>
      ) : null}
      {state.kind === 'complete' && !stale ? (
        <p>Creation is complete. Delivery and provider availability have not been checked.</p>
      ) : null}
      {!addressAvailable ? (
        <p>Recover the original channel address below to manage this record.</p>
      ) : null}
    </div>
  );
}
