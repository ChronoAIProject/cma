import {need} from './provenance.mjs';

// Parent invocation data never comes from an adapter-returned registration or
// mutable ctx.runState flags. Only invokeJourney installs a context.
const contexts = new WeakMap();
export function installContext(ctx, value) { contexts.set(ctx, value); }
export function contextFor(ctx) {
  const value = contexts.get(ctx);
  need(value, 'mfunc01_parent_invocation_required');
  return value;
}
