import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { ProtocolError } from './errors.js';
import { revocationReference, type RevocationResult } from './resource-revocation-result.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
} from './resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
} from './schema.js';

export const organizationRole = enumeration('admin', 'member', 'viewer');
export type OrganizationRole = Output<typeof organizationRole>;
export const organizationChoice = object({
  id: text,
  role: organizationRole,
  display_name: optional(text),
});
export type OrganizationChoice = Output<typeof organizationChoice>;

export const shareMode = enumeration('view', 'view_write', 'org_roles');
export const createShareRequest = object({ target_org_id: text, mode: shareMode });
export const updateShareRequest = object({ expected_revision: positiveInteger, mode: shareMode });
export type CreateShareRequest = Output<typeof createShareRequest>;
export type UpdateShareRequest = Output<typeof updateShareRequest>;
export const resourceOwner = tagged(
  {
    person: object({ kind: enumeration('person'), subject: text }),
    organization: object({ kind: enumeration('organization'), subject: text }),
  },
  'kind',
);
export const resourceActor = tagged(
  {
    human: object({ kind: enumeration('human'), subject: text }),
    api: object({ kind: enumeration('api'), subject: text, delegation_id: nullable(text) }),
    trigger: object({ kind: enumeration('trigger'), subject: text, launch_id: text }),
    channel: object({
      kind: enumeration('channel'),
      bridge_id: text,
      grant_id: text,
      platform: text,
      chat_id: text,
      sender_id: text,
    }),
  },
  'kind',
);
export const resourceSelector = tagged(
  {
    agent: object({ kind: enumeration('agent'), agent_id: text }),
    sandbox: object({ kind: enumeration('sandbox'), sandbox_id: text }),
  },
  'kind',
);
export const resourceShare = object({
  id: text,
  owner: resourceOwner,
  selector: resourceSelector,
  target_org_id: text,
  mode: shareMode,
  issuer_actor: resourceActor,
  created_at: text,
  revoked_at: nullable(text),
  revision: positiveInteger,
});
export const shareReceipt = object({
  operation_id: text,
  share: resourceShare,
  actor: resourceActor,
  at: text,
  policy_revision: positiveInteger,
});
/** Safe owner attribution; personal identities and ancestry are never exposed. */
export const resourceOwnership = tagged(
  {
    person: object({ kind: enumeration('person'), is_current_person: boolean }),
    organization: object({ kind: enumeration('organization'), organization_id: text }),
  },
  'kind',
);
export type ResourceOwnership = Output<typeof resourceOwnership>;
export const shareIdentity = object({
  subject: text,
  display_name: nullable(text),
  avatar_url: nullable(text),
});
export const agentShareAudience = tagged(
  {
    public: object({ kind: enumeration('public') }),
    organization: object({ kind: enumeration('organization'), organization_id: text }),
    users: object({ kind: enumeration('users'), subjects: array(text) }),
  },
  'kind',
);
export const agentShareLink = object({
  id: text,
  agent_id: text,
  audience: agentShareAudience,
  mode: enumeration('view', 'view_write'),
  shared_by: shareIdentity,
  created_at: text,
  revoked_at: nullable(text),
  revision: positiveInteger,
  accepted_count: integer,
});
export const createAgentShareLinkRequest = object({
  audience: agentShareAudience,
  mode: enumeration('view', 'view_write'),
  token: text,
});
export type ShareIdentity = Output<typeof shareIdentity>;
export type AgentShareAudience = Output<typeof agentShareAudience>;
export type AgentShareLink = Output<typeof agentShareLink>;
export type CreateAgentShareLinkRequest = Output<typeof createAgentShareLinkRequest>;
export const effectiveAccess = object({
  shared_by: optional(shareIdentity),
  owner: optional(resourceOwnership),
  capabilities: array(text),
  revalidate_at: nullable(text),
  policy_revision: integer,
});
export type ResourceShare = Output<typeof resourceShare>;
export type ShareReceipt = Output<typeof shareReceipt>;
export type EffectiveAccess = Output<typeof effectiveAccess>;

function changedShare(
  kind: string,
  id: string,
  shareId: string,
  revision: number,
  mode?: ResourceShare['mode'],
) {
  return (raw: unknown): ShareReceipt & RevocationResult => {
    const value = shareReceipt(raw);
    const revocation = revocationReference(raw, kind, id, shareId, revision);
    if (!revocation) return value;
    const selector = value.share.selector;
    if (
      value.operation_id !== revocation.operation ||
      value.share.id !== shareId ||
      value.share.revision !== revision + 1 ||
      selector.kind !== kind ||
      (selector.kind === 'agent' ? selector.agent_id : selector.sandbox_id) !== id ||
      (mode === undefined
        ? value.share.revoked_at === null
        : value.share.revoked_at !== null || value.share.mode !== mode)
    )
      throw new ProtocolError('Revocation receipt does not match the original request');
    return { ...value, revocation };
  };
}

/** Policy inspection and owner share management; a permission is not a later-operation credential. */
export class ResourceAccess {
  constructor(private readonly config: ResolvedConfig) {}
  /** Current organization choices; subsequent resource requests recheck access. */
  organizations(options: OperationOptions = {}) {
    return read(
      this.config,
      '/api/v1/organizations',
      object({ items: array(organizationChoice) }),
      options,
    );
  }
  effective(kind: string, id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/resource-access/${segment(kind)}/${segment(id)}`,
      effectiveAccess,
      options,
    );
  }
  listShares(kind: string, id: string, options: OperationOptions = {}) {
    return read(this.config, this.path(kind, id), object({ items: array(resourceShare) }), options);
  }
  readShare(kind: string, id: string, shareId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      this.path(kind, id, shareId),
      identity(resourceShare, shareId),
      options,
    );
  }
  createShare(kind: string, id: string, body: CreateShareRequest, options: CommandOptions) {
    return read(this.config, this.path(kind, id), shareReceipt, {
      ...command(options),
      method: 'POST',
      body: input(createShareRequest, body),
    });
  }
  updateShare(
    kind: string,
    id: string,
    shareId: string,
    body: UpdateShareRequest,
    options: CommandOptions,
  ) {
    return read(
      this.config,
      this.path(kind, id, shareId),
      changedShare(kind, id, shareId, body.expected_revision, body.mode),
      {
        ...command(options),
        method: 'PATCH',
        body: input(updateShareRequest, body),
      },
    );
  }
  revokeShare(
    kind: string,
    id: string,
    shareId: string,
    expectedRevision: number,
    options: CommandOptions,
  ) {
    input(positiveInteger, expectedRevision);
    return read(
      this.config,
      query(this.path(kind, id, shareId), { expected_revision: expectedRevision }),
      changedShare(kind, id, shareId, expectedRevision),
      { ...command(options), method: 'DELETE' },
    );
  }
  /** Active Agent-session links; raw link secrets are never returned. */
  listAgentShareLinks(agentId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/agents/${segment(agentId)}/share-links`,
      object({ items: array(agentShareLink) }),
      options,
    );
  }
  /** Generate token using 32 CSPRNG bytes (hex), retain it only in the URL fragment. */
  createAgentShareLink(
    agentId: string,
    body: CreateAgentShareLinkRequest,
    options: CommandOptions,
  ) {
    return read(this.config, `/api/v1/agents/${segment(agentId)}/share-links`, agentShareLink, {
      ...command(options),
      method: 'POST',
      body: input(createAgentShareLinkRequest, body),
    });
  }
  /** Revoke only this link and its recipients; other links remain independent. */
  revokeAgentShareLink(
    agentId: string,
    shareId: string,
    expectedRevision: number,
    options: CommandOptions,
  ) {
    input(positiveInteger, expectedRevision);
    return read(
      this.config,
      query(`/api/v1/agents/${segment(agentId)}/share-links/${segment(shareId)}`, {
        expected_revision: expectedRevision,
      }),
      (raw): AgentShareLink & RevocationResult => {
        const value = agentShareLink(raw);
        const revocation = revocationReference(raw, 'agent', agentId, shareId, expectedRevision);
        if (!revocation) return value;
        if (
          value.id !== shareId ||
          value.agent_id !== agentId ||
          value.revision !== expectedRevision + 1 ||
          value.revoked_at === null
        )
          throw new ProtocolError('Revocation link does not match the original request');
        return { ...value, revocation };
      },
      { ...command(options), method: 'DELETE' },
    );
  }
  /** Exchange a link secret for access bound to the current authenticated user. */
  redeemAgentShareLink(shareId: string, token: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/agent-share-links/${segment(shareId)}/redeem`,
      agentShareLink,
      { ...options, method: 'POST', body: { token } },
    );
  }
  /** Bounded canonical display-name lookup; only the owning Agent user can search. */
  shareRecipients(agentId: string, text: string, options: OperationOptions = {}) {
    return read(
      this.config,
      query(`/api/v1/agents/${segment(agentId)}/share-recipients`, { q: text }),
      object({ items: array(shareIdentity) }),
      options,
    );
  }
  private path(kind: string, id: string, shareId?: string) {
    return `/api/v1/resource-shares/${segment(kind)}/${segment(id)}${shareId === undefined ? '' : `/${segment(shareId)}`}`;
  }
}
