import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { input, query, read } from '../resources.js';
import * as schema from './types.js';
import type { ProfileAuthoringSkillSearch, ResolveRepositoryRequest } from './types.js';

export type {
  ProfileAuthoringBounds,
  ProfileAuthoringService,
  ProfileAuthoringSkill,
  ProfileAuthoringSkillPage,
  ProfileAuthoringSkillSearch,
  ResolveRepositoryRequest,
  ResolvedRepository,
} from './types.js';

/** Public read-only catalogs and repository verification for profile authors. */
export class ProfileAuthoring {
  readonly #config: ResolvedConfig;

  constructor(config: ResolvedConfig) {
    this.#config = config;
  }

  /** Immutable deployment capacity policy; requires profile-management capability. */
  bounds(options: OperationOptions = {}) {
    return read(this.#config, '/api/v1/profile-authoring/bounds', schema.bounds, options);
  }

  /** Safe service scope names, never connection credentials. */
  services(options: OperationOptions = {}) {
    return read(this.#config, '/api/v1/profile-authoring/services', schema.services, options);
  }

  /** One bounded skill page with exact version strings and an opaque cursor. */
  async searchSkills(search: ProfileAuthoringSkillSearch = {}, options: OperationOptions = {}) {
    input(schema.validateSearch, search);
    const path = query('/api/v1/profile-authoring/skills', {
      q: search.q,
      limit: search.limit,
      cursor: search.cursor,
    });
    return read(this.#config, path, (value) => schema.skills(value, search), options);
  }

  /** Read-only public GitHub verification; no idempotency key, clone, or work admission. */
  async resolveRepository(body: ResolveRepositoryRequest, options: OperationOptions = {}) {
    input(schema.repositoryRequest, body);
    return read(
      this.#config,
      '/api/v1/profile-authoring/repositories/resolve',
      (value) => schema.repository(value, body),
      {
        ...options,
        method: 'POST',
        body,
      },
    );
  }
}
