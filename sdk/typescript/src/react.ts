import { createElement, type ReactElement } from 'react';
import { ValidationError } from './errors.js';
import { input } from './resources.js';
import {
  triggerButtonLabel,
  triggerTheme,
  type TriggerButtonLabel,
  type TriggerTheme,
} from './trigger-authoring/types.js';

export interface CmaTriggerButtonProps {
  /** The published public id, not the owner's private Trigger id. */
  readonly triggerId: string;
  readonly placementId?: string;
  readonly label?: TriggerButtonLabel;
  readonly theme?: TriggerTheme;
  /** Absolute HTTP(S) CMA origin. Defaults to the production public origin. */
  readonly baseUrl?: string;
}

const labels: Readonly<Record<TriggerButtonLabel, string>> = {
  open_in_cma: 'Open in CMA',
  chat_with_this: 'Chat with this',
  run_with_cma: 'Run with CMA',
  try_it: 'Try it',
};

/** A server-renderable anchor and image. Rendering never calls an API or starts a Launch. */
export function CmaTriggerButton({
  triggerId,
  placementId,
  label = 'open_in_cma',
  theme = 'auto',
  baseUrl = 'https://cma.nyxid.ai',
}: CmaTriggerButtonProps): ReactElement {
  const identifier = (value: string) => {
    if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(value))
      throw new ValidationError('Trigger and placement ids must be valid public identifiers');
    return encodeURIComponent(value);
  };
  const trigger = identifier(triggerId);
  const placement = placementId === undefined ? undefined : identifier(placementId);
  input(triggerButtonLabel, label);
  input(triggerTheme, theme);
  let origin: URL;
  try {
    origin = new URL(baseUrl);
  } catch {
    throw new ValidationError('baseUrl must be an absolute HTTP(S) origin');
  }
  if (
    !['http:', 'https:'].includes(origin.protocol) ||
    origin.username ||
    origin.password ||
    origin.pathname !== '/' ||
    origin.search ||
    origin.hash
  )
    throw new ValidationError(
      'baseUrl must be an HTTP(S) origin without credentials, path, or query',
    );
  const href = new URL(
    placement === undefined
      ? `/t/${trigger}`
      : `/api/v1/triggers/public/${trigger}/open/${placement}`,
    origin,
  );
  const badge = new URL(`/api/v1/triggers/public/${trigger}/badge.svg`, origin);
  badge.searchParams.set('label', label);
  badge.searchParams.set('theme', theme);
  if (placementId !== undefined) badge.searchParams.set('placement', placementId);
  return createElement(
    'a',
    { href: href.href },
    createElement('img', { src: badge.href, alt: labels[label] }),
  );
}
