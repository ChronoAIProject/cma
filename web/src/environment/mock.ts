import type {
  CatalogSkill,
  EnvironmentProfile,
  EnvironmentProfileStore,
  MandatoryEntries,
  ProfileDraft,
} from './types';

/**
 * Stand-in profile store.
 *
 * The console has no live API client yet — its sessions are driven by a mock
 * driver and its identity by a mock provider — so the profile surface is
 * developed against this. It exists to prove the seam: when the REST client
 * lands it replaces this module and no component changes.
 *
 * It mirrors the server's non-revocable baseline rather than assuming the UI
 * will keep its hands off. That is deliberate: the guarantee under test is
 * "a client that omits them still gets them", and a fake that trusted the
 * caller would let a regression in the editor pass unnoticed.
 *
 * Nothing is written to localStorage or sessionStorage. A profile's values are
 * where a user keeps a token, and there is no path by which this could grow
 * into storing one on the device.
 */

/** What this deployment mandates, mirroring the control plane's configuration. */
const MANDATORY_PACKAGES = ['nyxid'] as const;
const MANDATORY_SKILLS = ['nyxid', 'ornn-agent-manual-cli'] as const;

/// Services every session reaches whatever a profile says.
const MANDATORY_SERVICES = ['chrono-llm', 'ornn-api'] as const;

/// What the picker offers, standing in for the owner's own account.
const SERVICES = [
  { id: 'chrono-llm', name: 'chrono-llm', description: 'The model endpoint' },
  { id: 'ornn-api', name: 'ornn-api', description: 'The skill registry' },
  { id: 'chrono-storage-service', name: 'chrono-storage-service', description: 'Object storage' },
  { id: 'api-github', name: 'api-github', description: 'GitHub' },
] as const;

const MANDATORY: MandatoryEntries = {
  packages: [...MANDATORY_PACKAGES],
  skills: [...MANDATORY_SKILLS],
};

const CATALOGUE: readonly CatalogSkill[] = [
  {
    id: 'nyxid',
    name: 'nyxid',
    description: 'Broker credentials for third-party services without handling a raw token.',
    access: 'public',
  },
  {
    id: 'ornn-agent-manual-cli',
    name: 'ornn-agent-manual-cli',
    description: 'Search, pull, run and publish skills from the registry.',
    access: 'public',
  },
  {
    id: 'aevatar-platform-map',
    name: 'aevatar-platform-map',
    description: 'Entry point and router for the agent platform family.',
    access: 'public',
  },
  {
    id: 'firecrawl-via-nyxid',
    name: 'firecrawl-via-nyxid',
    description: 'Run web research jobs through the credential broker.',
    access: 'shared-via-org',
  },
  {
    id: 'my-release-checklist',
    name: 'my-release-checklist',
    description: 'The release steps this account keeps to itself.',
    access: 'owner',
  },
];

let counter = 0;

/** Put the mandatory entries back, exactly as the server does. */
function applyBaseline(draft: ProfileDraft): {
  packages: EnvironmentProfile['packages'];
  skills: readonly string[];
} {
  const chosenPackages = draft.packages.filter((p) => p.name.trim().length > 0);
  const packages = [
    ...MANDATORY_PACKAGES.filter((name) => !chosenPackages.some((p) => p.name === name)).map(
      (name) => ({ name }),
    ),
    ...chosenPackages,
  ];
  const chosenSkills = draft.agent.skills.filter((s) => s.trim().length > 0);
  const skills = [...MANDATORY_SKILLS.filter((id) => !chosenSkills.includes(id)), ...chosenSkills];
  return { packages, skills };
}

function materialise(id: string, draft: ProfileDraft, createdAt: string): EnvironmentProfile {
  const { packages, skills } = applyBaseline(draft);
  return {
    id,
    name: draft.name.trim(),
    variables: [...draft.variables],
    packages,
    agent: { ...draft.agent, skills },
    nyxid: { services: [...draft.nyxid.services] },
    mandatory: MANDATORY,
    createdAt,
    updatedAt: new Date().toISOString(),
  };
}

export interface MockProfileOptions {
  /** Start with these profiles already stored. */
  readonly initial?: readonly ProfileDraft[];
  /** Force every call to fail, so error paths can be exercised. */
  readonly failWith?: string;
  /** Force the skill registry alone to fail, which is a recoverable state. */
  readonly failSkillsWith?: string;
}

export function createMockProfileStore(options: MockProfileOptions = {}): EnvironmentProfileStore {
  const stored: EnvironmentProfile[] = (options.initial ?? []).map((draft) => {
    counter += 1;
    return materialise(`envp_${counter.toString(16)}`, draft, new Date().toISOString());
  });

  function guard(): void {
    if (options.failWith !== undefined) {
      throw new Error(options.failWith);
    }
  }

  return {
    async list() {
      guard();
      return [...stored].reverse();
    },

    async create(draft) {
      guard();
      counter += 1;
      const profile = materialise(`envp_${counter.toString(16)}`, draft, new Date().toISOString());
      stored.push(profile);
      return profile;
    },

    async update(id, draft) {
      guard();
      const index = stored.findIndex((p) => p.id === id);
      if (index === -1) {
        throw new Error('That profile no longer exists. Reopen the list to see what is there.');
      }
      const existing = stored[index];
      const replaced = materialise(id, draft, existing?.createdAt ?? new Date().toISOString());
      stored[index] = replaced;
      return replaced;
    },

    async remove(id) {
      guard();
      const index = stored.findIndex((p) => p.id === id);
      if (index === -1) {
        throw new Error('That profile no longer exists.');
      }
      stored.splice(index, 1);
    },

    async services() {
      guard();
      return { services: SERVICES, mandatory: MANDATORY_SERVICES };
    },

    async catalogue() {
      guard();
      if (options.failSkillsWith !== undefined) {
        throw new Error(options.failSkillsWith);
      }
      return { skills: CATALOGUE, mandatory: MANDATORY };
    },
  };
}
