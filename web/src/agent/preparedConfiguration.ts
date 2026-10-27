import { useLayoutEffect, useRef } from 'react';
import type { AgentResourceState } from './controller';

/** Prepared consent is reusable only for the observed resource configuration. */
export function usePreparedConfiguration(
  state: AgentResourceState,
  scope: object,
  discardPrepared: () => void,
) {
  const connectionConfiguration =
    state.status === 'ready'
      ? JSON.stringify([state.detail.sandbox.id, state.detail.sandbox.profile, state.detail.reach])
      : undefined;
  const observedConfiguration = useRef<{ scope: object; value: string } | undefined>(undefined);
  useLayoutEffect(() => {
    if (connectionConfiguration === undefined) return;
    if (
      observedConfiguration.current?.scope === scope &&
      observedConfiguration.current.value !== connectionConfiguration
    )
      discardPrepared();
    observedConfiguration.current = { scope: scope, value: connectionConfiguration };
  }, [connectionConfiguration, scope, discardPrepared]);
}
