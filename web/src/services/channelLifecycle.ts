export type ChannelCreationState =
  | { readonly kind: 'complete' }
  | { readonly kind: 'incomplete' }
  | { readonly kind: 'legacy' }
  | { readonly kind: 'unconfirmed' };

export function classifyChannelLifecycle(definition: {
  readonly creation?: unknown;
}): ChannelCreationState {
  if (!Object.hasOwn(definition, 'creation')) return { kind: 'legacy' };
  const value = definition.creation;
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    return { kind: 'unconfirmed' };
  if (!('ready' in value) || typeof value.ready !== 'boolean') return { kind: 'unconfirmed' };
  return { kind: value.ready ? 'complete' : 'incomplete' };
}
