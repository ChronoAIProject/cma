import type { ResolvedConfig } from '../config.js';
import { ProtocolError, TransportError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, segment } from '../resources.js';
import { eventStream } from '../streaming.js';
import { cursor, identifier, pageCursor, type V2AgentCursor } from './schema.js';
import { readAgentJson } from './read.js';
import {
  externalApprovalProjection,
  externalApprovalPreference,
  type ExternalApprovalProjection,
} from './external-approval.js';
export type { ExternalApprovalProjection } from './external-approval.js';
import { agentFrames } from './stream.js';
import {
  v2AgentEventHead,
  v2AgentEventPage,
  v2UiTransport,
  type V2AgentStreamFrame,
} from './types.js';
export type * from './types.js';
export type { V2AgentEventKind } from './kinds.js';
export type { V2AgentCursor, V2AgentPageCursor } from './schema.js';

export interface V2AgentProjectionOptions extends OperationOptions {
  readonly externalApprovals?: boolean;
  readonly onExternalApprovalProjection?: (projection: ExternalApprovalProjection) => void;
}
export interface V2AgentSnapshotOptions extends V2AgentProjectionOptions {
  readonly toolOutput?: boolean;
  /** Request typed retained partial-output discovery; older servers may omit it. */
  readonly retainedOutput?: boolean;
  /** Opt into authoritative per-message command correlation; older servers may omit it. */
  readonly messageBindings?: boolean;
  readonly limit?: number;
  readonly pageCursor?: string;
}
export interface V2AgentEventsOptions extends V2AgentProjectionOptions {
  readonly toolOutput?: boolean;
  /** Request typed retained partial-output discovery; older servers may omit it. */
  readonly retainedOutput?: boolean;
  /** Request the server's real live-fragment observation time; older servers may omit it. */
  readonly liveTimestamps?: boolean;
  /** Opt into authoritative per-message command correlation; older servers may omit it. */
  readonly messageBindings?: boolean;
  /** The last completely applied durable Agent cursor; omit to start at the retained beginning. */
  readonly afterCursor?: string;
  /** Standard SSE resume header alternative; if both coordinates are supplied they must match. */
  readonly lastEventId?: string;
  readonly follow?: boolean;
  /** Apply the complete frame. A rejected callback never acknowledges that frame. */
  readonly onEvent: (frame: V2AgentStreamFrame) => void | Promise<void>;
  /** Persist only after onEvent succeeds. Ephemeral frames never invoke this callback. */
  readonly onCursor?: (cursor: V2AgentCursor) => void | Promise<void>;
}
/** Bounded rich Agent replay, independent of standard Responses stream sequences. */
export class AgentEventsV2 {
  constructor(private readonly config: ResolvedConfig) {}
  /** Authenticated eligibility for choosing a new tab's transport; never command authority. */
  async readTransportEligibility(options: OperationOptions = {}) {
    return v2UiTransport(await readAgentJson(this.config, '/api/v2/ui/transport', options));
  }
  /** Capture a current tail baseline while historical snapshot pages load independently. */
  async head(agentId: string, options: V2AgentProjectionOptions = {}) {
    const id = preflight(identifier, agentId);
    let projection: ExternalApprovalProjection = { supported: false, canReview: false };
    const value = await readAgentJson(
      this.config,
      `/api/v2/agents/${segment(id)}/events/head`,
      options,
      options.externalApprovals ? { Prefer: externalApprovalPreference } : undefined,
      (headers) => {
        projection = externalApprovalProjection(headers, options.externalApprovals === true);
      },
    );
    const result = v2AgentEventHead(value, id);
    options.onExternalApprovalProjection?.(projection);
    return options.externalApprovals
      ? { ...result, externalApprovalProjection: projection }
      : result;
  }

  /** Fetch one fixed-head page. Apply every page before acknowledging through_cursor for tail startup. */
  async snapshot(agentId: string, options: V2AgentSnapshotOptions = {}) {
    const id = preflight(identifier, agentId);
    const previous =
      options.pageCursor === undefined ? undefined : preflight(pageCursor, options.pageCursor);
    const limit = options.limit ?? 100;
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200)
      throw new ValidationError('Agent snapshot limit must be 1 through 200');
    let projection: ExternalApprovalProjection = { supported: false, canReview: false };
    const value = await readAgentJson(
      this.config,
      query(`/api/v2/agents/${segment(id)}/events/snapshot`, {
        limit: options.limit,
        page_cursor: previous,
      }),
      options,
      options.externalApprovals ||
        options.messageBindings ||
        options.toolOutput ||
        options.retainedOutput
        ? {
            Prefer: [
              options.externalApprovals ? externalApprovalPreference : undefined,
              options.messageBindings ? 'cma-message-bindings' : undefined,
              options.toolOutput ? 'cma-tool-output' : undefined,
              options.retainedOutput ? 'cma-retained-output' : undefined,
            ]
              .filter(Boolean)
              .join(', '),
          }
        : undefined,
      (headers) => {
        projection = externalApprovalProjection(headers, options.externalApprovals === true);
      },
    );
    const result = v2AgentEventPage(value, id, limit, previous, projection.supported);
    options.onExternalApprovalProjection?.(projection);
    return options.externalApprovals
      ? { ...result, externalApprovalProjection: projection }
      : result;
  }
  /** Open one read-only tail. Reconnect explicitly with the last successfully persisted Agent cursor. */
  async events(agentId: string, options: V2AgentEventsOptions): Promise<void> {
    const id = preflight(identifier, agentId);
    const queryCursor =
      options.afterCursor === undefined ? undefined : preflight(cursor, options.afterCursor);
    const headerCursor =
      options.lastEventId === undefined ? undefined : preflight(cursor, options.lastEventId);
    if (queryCursor !== undefined && headerCursor !== undefined && queryCursor !== headerCursor)
      throw new ValidationError('afterCursor and Last-Event-ID must agree');
    const after = queryCursor ?? headerCursor;
    if (options.follow !== undefined && typeof options.follow !== 'boolean')
      throw new ValidationError('follow must be a boolean');
    if (options.liveTimestamps !== undefined && typeof options.liveTimestamps !== 'boolean')
      throw new ValidationError('liveTimestamps must be a boolean');
    if (
      typeof options.onEvent !== 'function' ||
      (options.onCursor !== undefined && typeof options.onCursor !== 'function')
    )
      throw new ValidationError('Agent event consumer must be a function');
    const path = query(`/api/v2/agents/${segment(id)}/events`, {
      after_cursor: queryCursor,
      follow: options.follow === undefined ? undefined : String(options.follow),
    });
    for await (const frame of eventStream(
      this.config,
      path,
      options,
      (response, signal) => {
        const projection = externalApprovalProjection(
          response.headers,
          options.externalApprovals === true,
        );
        options.onExternalApprovalProjection?.(projection);
        return agentFrames(response, signal, id, after, projection.supported);
      },
      'Agent events v2',
      undefined,
      {
        ...(headerCursor === undefined ? {} : { 'Last-Event-ID': headerCursor }),
        ...(options.externalApprovals ||
        options.messageBindings ||
        options.liveTimestamps ||
        options.toolOutput ||
        options.retainedOutput
          ? {
              Prefer: [
                options.externalApprovals ? externalApprovalPreference : undefined,
                options.messageBindings ? 'cma-message-bindings' : undefined,
                options.liveTimestamps ? 'cma-live-observed-at' : undefined,
                options.toolOutput ? 'cma-tool-output' : undefined,
                options.retainedOutput ? 'cma-retained-output' : undefined,
              ]
                .filter(Boolean)
                .join(', '),
            }
          : {}),
      },
    )) {
      try {
        await options.onEvent(frame);
        if (frame.type === 'cma.agent.event') await options.onCursor?.(frame.data.cursor);
      } catch (cause) {
        throw new ProtocolError('Agent consumer did not acknowledge a complete frame', { cause });
      }
      if (frame.type === 'cma.agent.error')
        throw new TransportError(
          `Agent observation ended: ${frame.data.code}: ${frame.data.message}`,
        );
    }
  }
}
function preflight<T>(parse: (value: unknown) => T, value: unknown): T {
  try {
    return parse(value);
  } catch (cause) {
    throw new ValidationError('invalid Agent event request', { cause });
  }
}
