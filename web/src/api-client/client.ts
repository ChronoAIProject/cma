import { AccountDeletion } from './account-deletion.js';
import { PrivacyExports } from './privacy-export.js';
import { LegalConsents } from './legal-consent.js';
import { reportBrowserFault, type BrowserFault } from './browser-telemetry.js';
import { Documents } from './documents.js';
import { TaskExperiencePolicy } from './task-experience.js';
import { SandboxUsage } from './usage.js';
import { SandboxMetrics } from './usage-metrics/index.js';
import { EventWorkerProfiles } from './event-worker-profiles.js';
import { ProviderBindings } from './provider-bindings.js';
import { GatewayRecords } from './gateway-records.js';
import { Bots } from './bots.js';
import { BotSetups } from './bot-setups.js';
import { AgentDelegations } from './agent-delegations.js';
import { GatewayInteractions } from './gateway-interactions.js';
import { readAgentCard } from './agent-card.js';
import { resolveConfig, type ClientConfig, type ResolvedConfig } from './config.js';
import {
  readConformance,
  readOpenApi,
  type Conformance,
  type OpenApiDocument,
} from './discovery.js';
import type { OperationOptions } from './http.js';
import { Agents } from './agents.js';
import { AgentAttachments } from './agents/attachments.js';
import { AgentFiles } from './agent-files.js';
import { AgentBrowser } from './agent-browser.js';
import { AgentBrowserControl } from './agent-browser-control.js';
import { Marketplace } from './marketplace.js';
import { Profiles } from './profiles.js';
import { Sandboxes } from './sandboxes.js';
import { Workspaces, Worktrees } from './workspaces.js';
import { readDefaults, type PlatformDefaults } from './defaults.js';
import { Administration } from './admin/index.js';
import { TriggerAuthoring } from './trigger-authoring/index.js';
import { AgentConversations } from './agui/index.js';
import { Launches } from './launch/index.js';
import { Authentication } from './auth.js';
import { ResponsesV2 } from './responses-v2.js';
import { Responses } from './responses.js';
import { UnsupportedInterfaces } from './unsupported.js';
import { ProfileAuthoring } from './profile-authoring/index.js';
import { ResourceAccess } from './resource-access.js';
import { ResourceRevocations } from './resource-revocations.js';
import { AgentEventsV2 } from './agent-events-v2/index.js';
import { RuntimeCanaries } from './runtime-canary.js';
import { RetainedRuntimeTransitions } from './retained-runtime-transition.js';

/** A configured client for the public CMA resource and discovery APIs. */
export class Cma {
  readonly #config: ResolvedConfig;
  readonly documents: Documents;
  readonly accountDeletion: AccountDeletion;
  readonly privacyExports: PrivacyExports;
  readonly legalConsent: LegalConsents;
  readonly taskExperience: TaskExperiencePolicy;
  readonly usage: SandboxUsage;
  readonly usageMetrics: SandboxMetrics;
  readonly adminUsageMetrics: SandboxMetrics;
  readonly providerBindings: ProviderBindings;
  readonly gatewayRecords: GatewayRecords;
  readonly gatewayConnectionRecords: GatewayRecords;
  readonly bots: Bots;
  readonly botSetups: BotSetups;
  readonly agentDelegations: AgentDelegations;
  readonly gatewayInteractions: GatewayInteractions;
  readonly eventWorkerProfiles: EventWorkerProfiles;
  readonly agents: Agents;
  readonly agentAttachments: AgentAttachments;
  readonly agentFiles: AgentFiles;
  readonly agentBrowser: AgentBrowser;
  readonly agentBrowserControl: AgentBrowserControl;
  readonly resourceAccess: ResourceAccess;
  readonly resourceRevocations: ResourceRevocations;
  readonly sandboxes: Sandboxes;
  readonly workspaces: Workspaces;
  readonly worktrees: Worktrees;
  readonly admin: Administration;
  readonly runtimeCanaries: RuntimeCanaries;
  readonly retainedRuntime: RetainedRuntimeTransitions;
  readonly profileAuthoring: ProfileAuthoring;
  readonly triggerAuthoring: TriggerAuthoring;
  readonly triggers: TriggerAuthoring;
  readonly auth: Authentication;
  readonly responses: Responses;
  readonly responsesV2: ResponsesV2;
  readonly agentEventsV2: AgentEventsV2;
  readonly unsupported: UnsupportedInterfaces;
  readonly agui: AgentConversations;
  readonly launches: Launches;
  readonly agentProfiles: Profiles<'agent'>;
  readonly workspaceProfiles: Profiles<'workspace'>;
  readonly sandboxProfiles: Profiles<'sandbox'>;
  readonly marketplace: {
    readonly agent: Marketplace<'agent'>;
    readonly workspace: Marketplace<'workspace'>;
    readonly sandbox: Marketplace<'sandbox'>;
  };

  constructor(config: ClientConfig = {}) {
    this.#config = resolveConfig(config);
    this.documents = new Documents(this.#config);
    this.accountDeletion = new AccountDeletion(this.#config);
    this.privacyExports = new PrivacyExports(this.#config);
    this.legalConsent = new LegalConsents(this.#config);
    this.taskExperience = new TaskExperiencePolicy(this.#config);
    this.usage = new SandboxUsage(this.#config);
    this.usageMetrics = new SandboxMetrics(this.#config);
    this.adminUsageMetrics = new SandboxMetrics(this.#config, true);
    this.providerBindings = new ProviderBindings(this.#config);
    this.gatewayRecords = new GatewayRecords(this.#config);
    this.gatewayConnectionRecords = new GatewayRecords(this.#config, 'connection-records');
    this.bots = new Bots(this.#config);
    this.botSetups = new BotSetups(this.#config);
    this.agentDelegations = new AgentDelegations(this.#config);
    this.gatewayInteractions = new GatewayInteractions(this.#config);
    this.eventWorkerProfiles = new EventWorkerProfiles(this.#config);
    this.agents = new Agents(this.#config);
    this.agentAttachments = new AgentAttachments(this.#config);
    this.agentFiles = new AgentFiles(this.#config);
    this.agentBrowser = new AgentBrowser(this.#config);
    this.agentBrowserControl = new AgentBrowserControl(this.#config);
    this.resourceAccess = new ResourceAccess(this.#config);
    this.resourceRevocations = new ResourceRevocations(this.#config);
    this.sandboxes = new Sandboxes(this.#config);
    this.workspaces = new Workspaces(this.#config);
    this.worktrees = new Worktrees(this.#config);
    this.admin = new Administration(this.#config);
    this.runtimeCanaries = new RuntimeCanaries(this.#config);
    this.retainedRuntime = new RetainedRuntimeTransitions(this.#config);
    this.profileAuthoring = new ProfileAuthoring(this.#config);
    this.triggerAuthoring = new TriggerAuthoring(this.#config);
    this.triggers = this.triggerAuthoring;
    this.auth = new Authentication(this.#config);
    this.responses = new Responses(this.#config);
    this.responsesV2 = new ResponsesV2(this.#config);
    this.agentEventsV2 = new AgentEventsV2(this.#config);
    this.unsupported = new UnsupportedInterfaces(this.#config);
    this.agui = new AgentConversations(this.#config);
    this.launches = new Launches(this.#config);
    this.agentProfiles = new Profiles(this.#config, 'agent');
    this.workspaceProfiles = new Profiles(this.#config, 'workspace');
    this.sandboxProfiles = new Profiles(this.#config, 'sandbox');
    this.marketplace = {
      agent: new Marketplace(this.#config, 'agent'),
      workspace: new Marketplace(this.#config, 'workspace'),
      sandbox: new Marketplace(this.#config, 'sandbox'),
    };
  }

  /** The resolved transport configuration held by this client. */
  get config(): ResolvedConfig {
    return this.#config;
  }

  /** Send one anonymous, content-free diagnostic; never a durable success receipt. */
  reportBrowserFault(kind: BrowserFault, options: OperationOptions = {}): Promise<void> {
    return reportBrowserFault(this.#config, kind, options);
  }

  /** Read the anonymous deployment state. */
  readConformance(options: OperationOptions = {}): Promise<Conformance> {
    return readConformance(this.#config, options);
  }

  readOpenApi(options: OperationOptions = {}): Promise<OpenApiDocument> {
    return readOpenApi(this.#config, options);
  }

  readAgentCard(options: OperationOptions = {}) {
    return readAgentCard(this.#config, options);
  }

  readDefaults(options: OperationOptions = {}): Promise<PlatformDefaults> {
    return readDefaults(this.#config, options);
  }
}

/** Construct a configured CMA resource client. */
export function createClient(config: ClientConfig = {}): Cma {
  return new Cma(config);
}
