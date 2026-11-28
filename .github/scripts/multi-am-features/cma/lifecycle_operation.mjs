import {poll} from './core.mjs';
import {lifecyclePollFailure} from '../failure.mjs';

export function pollLifecycleOperation(read, accept, {manager, onTransient, ...options}) {
  let missed = false;
  return poll(async () => {
    try {
      return await read();
    } catch (error) {
      const failure = lifecyclePollFailure(error, manager);
      if (missed || !(failure.kind === 'request_timeout'
          || (failure.kind === 'transport_error' && failure.cause === 'UND_ERR_SOCKET'))) throw error;
      missed = true;
      await onTransient(failure);
      return null;
    }
  }, value => value !== null && accept(value), options);
}
