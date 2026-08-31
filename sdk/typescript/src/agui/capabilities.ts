/** The deployment-level AG-UI features a client must discover rather than infer. */

import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import { request, type OperationOptions } from '../http.js';

export interface AguiCapabilityDocument {
  readonly protocol: 'ag-ui';
  readonly features: {
    /** Vendor extensions retain their declared names and additive values. */
    readonly custom: Readonly<Record<string, unknown>>;
  };
}

function object(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return value as Record<string, unknown>;
}

/** Validate the stable boundary needed to inspect additive custom features. */
export function toAguiCapabilities(raw: unknown): AguiCapabilityDocument {
  const document = object(raw, 'AG-UI capability document');
  if (document.protocol !== 'ag-ui') {
    throw new ProtocolError('AG-UI capability document did not name the ag-ui protocol');
  }
  const features = object(document.features, 'AG-UI capability features');
  const custom = object(features.custom, 'AG-UI custom capabilities');
  return { protocol: 'ag-ui', features: { custom } };
}

/** Read the live capability document. */
export async function readAguiCapabilities(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<AguiCapabilityDocument> {
  return toAguiCapabilities(await request(config, '/api/v1/agui/capabilities', options));
}
