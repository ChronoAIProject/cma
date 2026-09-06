import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import type { OperationOptions } from './http.js';
import {
  command,
  identity,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
  type ResourcePage,
} from './resources.js';
import { positiveInteger, type Parser } from './schema.js';
import {
  contentParsers,
  profileDocument,
  profileDraft,
  profileRevision,
  profileVisibility,
  type ProfileContents,
  type ProfileDocument,
  type ProfileDraft,
  type ProfileKind,
  type ProfileRevision,
  type ProfileVisibility,
} from './profiles/types.js';

export * from './profiles/types.js';

/** Owner-facing drafts and immutable revisions for one profile kind. */
export class Profiles<Kind extends ProfileKind> {
  readonly #path: string;
  readonly #document: Parser<ProfileDocument<ProfileContents[Kind]>>;
  readonly #revision: Parser<ProfileRevision<ProfileContents[Kind]>>;
  readonly #draft: Parser<ProfileDraft<ProfileContents[Kind]>>;

  constructor(
    private readonly config: ResolvedConfig,
    readonly kind: Kind,
  ) {
    this.#path = `/api/v1/${kind}-profiles`;
    this.#document = profileDocument(contentParsers[kind]);
    this.#revision = profileRevision(contentParsers[kind]);
    this.#draft = profileDraft(contentParsers[kind]);
  }

  list(options: PageOptions = {}): Promise<ResourcePage<ProfileDocument<ProfileContents[Kind]>>> {
    return read(
      this.config,
      query(this.#path, { cursor: options.cursor, limit: options.limit }),
      page(this.#document),
      options,
    );
  }

  read(
    id: string,
    options: OperationOptions = {},
  ): Promise<ProfileDocument<ProfileContents[Kind]>> {
    return read(this.config, `${this.#path}/${segment(id)}`, identity(this.#document, id), options);
  }

  create(
    body: ProfileDraft<ProfileContents[Kind]>,
    options: CommandOptions,
  ): Promise<ProfileDocument<ProfileContents[Kind]>> {
    return read(this.config, this.#path, this.#document, {
      ...command(options),
      method: 'POST',
      body: input(this.#draft, body),
      expectedStatus: 201,
    });
  }

  save(
    id: string,
    body: ProfileDraft<ProfileContents[Kind]> & { readonly draft_version: number },
    options: CommandOptions,
  ): Promise<ProfileDocument<ProfileContents[Kind]>> {
    input(positiveInteger, body.draft_version);
    input(this.#draft, body);
    return read(this.config, `${this.#path}/${segment(id)}`, identity(this.#document, id), {
      ...command(options),
      method: 'PUT',
      body,
    });
  }

  publish(
    id: string,
    body: { readonly draft_version: number },
    options: CommandOptions,
  ): Promise<ProfileRevision<ProfileContents[Kind]>> {
    input(positiveInteger, body.draft_version);
    return read(this.config, `${this.#path}/${segment(id)}/revisions`, this.revisionIdentity(id), {
      ...command(options),
      method: 'POST',
      body,
      expectedStatus: 201,
    });
  }

  listRevisions(
    id: string,
    options: PageOptions = {},
  ): Promise<ResourcePage<ProfileRevision<ProfileContents[Kind]>>> {
    return read(
      this.config,
      query(`${this.#path}/${segment(id)}/revisions`, {
        cursor: options.cursor,
        limit: options.limit,
      }),
      page(this.revisionIdentity(id)),
      options,
    );
  }

  readRevision(
    id: string,
    revision: number,
    options: OperationOptions = {},
  ): Promise<ProfileRevision<ProfileContents[Kind]>> {
    if (!Number.isSafeInteger(revision) || revision <= 0)
      throw new ValidationError('revision must be a positive safe integer');
    return read(
      this.config,
      `${this.#path}/${segment(id)}/revisions/${String(revision)}`,
      this.revisionIdentity(id, revision),
      options,
    );
  }

  setVisibility(
    id: string,
    body: { readonly visibility: ProfileVisibility },
    options: CommandOptions,
  ): Promise<ProfileDocument<ProfileContents[Kind]>> {
    input(profileVisibility, body.visibility);
    return read(
      this.config,
      `${this.#path}/${segment(id)}/visibility`,
      identity(this.#document, id),
      { ...command(options), method: 'PUT', body },
    );
  }

  retire(id: string, options: CommandOptions): Promise<ProfileDocument<ProfileContents[Kind]>> {
    return read(this.config, `${this.#path}/${segment(id)}/retire`, identity(this.#document, id), {
      ...command(options),
      method: 'POST',
    });
  }

  private revisionIdentity(
    id: string,
    revision?: number,
  ): Parser<ProfileRevision<ProfileContents[Kind]>> {
    return (value) => {
      const record = this.#revision(value);
      if (record.profile_id !== id || (revision !== undefined && record.revision !== revision))
        throw new ProtocolError('profile revision identity did not match the request');
      return record;
    };
  }
}
