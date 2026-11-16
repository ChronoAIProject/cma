import { randomUUID, createHash } from 'node:crypto';
import { Blocked } from '../../runner.mjs';
const hash = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex');
const enc = encodeURIComponent;
const requireValue = (value, code) => {
  if (!value) throw Error(code);
};
const supported = [
  'sandbox-always-on',
  'sandbox-delete',
  'sandbox-overview',
  'workspace-empty',
  'workspace-git',
  'workspace-delete',
];

// The parent supplies its existing authenticated transport and read-only
// infrastructure observer. These exports never start an application or seed a DB.
// Setup traffic is recorded separately and must not count as feature execution.
export function createUserFixtureFactories({ config, request, finality, readFault }) {
  const factories = Object.fromEntries(supported.map((name) => [name, (ctx) => take(name, ctx)]));
  async function take(name, ctx) {
    if (
      config?.scope !== 'LOCAL_DISPOSABLE_TWO_AM' ||
      config.database_disposable !== true ||
      config.single_matrix_writer !== true ||
      typeof request !== 'function' ||
      typeof finality !== 'function'
    )
      throw new Blocked('local_setup_services_unavailable');
    const profile = config.sandbox_profile;
    if (!profile?.id || !Number.isSafeInteger(profile.revision) || profile.revision < 1)
      throw new Blocked('sandbox_profile_fixture_unavailable');
    if (name === 'sandbox-always-on' && config.admin_available !== true)
      throw new Blocked('sandbox_admin_fixture_unavailable');
    if (name === 'sandbox-overview' && typeof readFault !== 'function')
      throw new Blocked('sandbox_read_fault_unavailable');
    if (
      name === 'workspace-git' &&
      (!config.repository?.url ||
        !config.repository?.ref ||
        !/^[a-f0-9]{40}$/.test(config.repository?.commit ?? ''))
    )
      throw new Blocked('local_git_fixture_unavailable');

    const ids = new Map(),
      uncertain = new Set(),
      children = new Set();
    const am = ctx.am,
      opposite = am === 'a' ? 'b' : 'a';
    const timeout = config.setup_timeout_ms ?? 240000;
    let complete, cleanupPromise;
    async function call(method, path, body, actor = 'user', ingress = am, key) {
      const value = await request({ actor, am: ingress, method, path, body, key });
      requireValue(
        value?.timing?.am === ingress &&
          typeof value.timing.request_id === 'string' &&
          value.timing.request_id.trim(),
        'setup_request_attribution',
      );
      await ctx.observe('fixture_request', async () => ({
        method,
        path_sha256: hash(path),
        status: value.status,
        am: value.timing.am,
        request_id: value.timing.request_id,
      }));
      requireValue(value.status >= 200 && value.status < 300, 'setup_request_failed');
      return value.body;
    }
    async function wait(path, terminal, failure = []) {
      const until = Date.now() + timeout;
      do {
        const value = await call('GET', path, undefined, 'user', opposite);
        requireValue(!failure.includes(value.status), 'fixture_resource_failed');
        if (terminal.includes(value.status)) return value;
        if (Date.now() >= until) break;
        await new Promise((resolve) => setTimeout(resolve, 2000));
      } while (Date.now() < until);
      throw Error('fixture_resource_deadline');
    }
    async function create(kind, path, body) {
      const key = `coverage06-setup-${randomUUID()}`;
      uncertain.add(key);
      // Preserve custody even when a successful write has invalid attribution.
      // Never issue another creation to replace an uncertain response.
      let response;
      try {
        response = await request({ actor: 'user', am, method: 'POST', path, body, key });
        if (response?.body?.id && typeof response.body.id === 'string') {
          ids.set(response.body.id, kind);
          uncertain.delete(key);
        }
        requireValue(response?.status === 201 || response?.status === 202, 'fixture_creation_status');
        requireValue(ids.has(response.body?.id), 'fixture_creation_identity');
        requireValue(
          response.timing?.am === am && response.timing.request_id?.trim(),
          'setup_request_attribution',
        );
      } catch (error) {
        const known = error?.response?.body?.id;
        if (typeof known === 'string') {
          ids.set(known, kind);
          uncertain.delete(key);
        }
        throw error;
      } finally {
        await ctx.observe('fixture_creation_custody', async () => ({
          kind,
          key_sha256: hash(key),
          resource_sha256: response?.body?.id ? hash(response.body.id) : null,
          request_id: response?.timing?.request_id ?? null,
          unresolved: uncertain.has(key),
        }));
      }
      return response.body;
    }
    async function cleanupOnce() {
      const failures = [];
      for (const [id, kind] of ids) {
        if (kind !== 'sandbox') continue;
        try {
          const path = '/api/v1/sandboxes/' + enc(id);
          const state = await call('GET', path, undefined, 'user', opposite);
          if (!['retiring', 'retired'].includes(state.status))
            await call('DELETE', path, undefined, 'user', opposite, `coverage06-cleanup-${randomUUID()}`);
          await wait(path, ['retired']);
          const observed = await finality({ sandboxId: id });
          requireValue(
            observed?.sandbox_id === id &&
              observed.children_remaining === 0 &&
              observed.cleanup_complete === true,
            'fixture_cleanup_children_remaining',
          );
        } catch (error) {
          failures.push(error);
          await ctx.observe('fixture_cleanup_failure', async () => ({
            resource_sha256: hash(id),
            code: /^[a-z][a-z0-9_]+$/.test(error?.message ?? '') ? error.message : 'fixture_cleanup_failed',
          }));
        }
      }
      if (failures.length) throw new AggregateError(failures, 'fixture_cleanup_children_remaining');
      requireValue(uncertain.size === 0, 'fixture_uncertain_creation_requires_parent');
      complete = {
        complete: true,
        owned_only: true,
        resource_ids_sha256: hash([...ids]),
        registered_children_sha256: hash([...children]),
      };
      return complete;
    }
    const cleanup = () => (complete ? Promise.resolve(complete) : (cleanupPromise ??= cleanupOnce()));
    ctx.defer('cleanup_partial_user_setup', cleanup);
    const primary = await create('sandbox', '/api/v1/sandboxes', {
      sandbox_profile: profile,
      name: `coverage06-${randomUUID()}`,
    });
    const current = await wait('/api/v1/sandboxes/' + enc(primary.id), ['active'], ['failed', 'retired']);
    const result = {
      local: true,
      run_owned: true,
      sandboxId: primary.id,
      sandboxName: current.name,
      disposable: true,
      cleanup,
      registerWorkspace: async (id) => {
        const child = await call('GET', '/api/v1/workspaces/' + enc(id));
        requireValue(child.sandbox_id === primary.id, 'fixture_child_parent');
        children.add(id);
      },
      resourceFinality: () => finality({ sandboxId: primary.id, workspaceId: result.workspaceId }),
    };
    if (name === 'sandbox-always-on') {
      const approved = await call(
        'POST',
        `/api/v1/admin/sandboxes/${enc(primary.id)}/always-on`,
        { always_on: true, expected_revision: current.revision },
        'admin',
      );
      requireValue(approved.id === primary.id && approved.always_on === true, 'fixture_approval_failed');
    }
    if (name === 'sandbox-delete') {
      const sibling = await create('sandbox', '/api/v1/sandboxes', {
        sandbox_profile: profile,
        name: `coverage06-sibling-${randomUUID()}`,
      });
      const ready = await wait('/api/v1/sandboxes/' + enc(sibling.id), ['active'], ['failed']);
      Object.assign(result, { siblingId: sibling.id, siblingStatus: ready.status });
    }
    if (name === 'sandbox-overview' || name === 'workspace-delete') {
      const workspace = await create('workspace', `/api/v1/sandboxes/${enc(primary.id)}/workspaces/inline`, {
        name: 'coverage06-workspace',
        source: { type: 'empty' },
      });
      const ready = await wait('/api/v1/workspaces/' + enc(workspace.id), ['ready'], ['failed']);
      Object.assign(result, { workspaceId: workspace.id, workspaceName: ready.name });
      if (name === 'sandbox-overview')
        result.failNextRead = (label) =>
          readFault({
            sandboxId: primary.id,
            path: '/api/v1/sandboxes/' + enc(primary.id),
            label,
          });
      else {
        const sibling = await create('workspace', `/api/v1/sandboxes/${enc(primary.id)}/workspaces/inline`, {
          name: 'coverage06-sibling-workspace',
          source: { type: 'empty' },
        });
        const readySibling = await wait('/api/v1/workspaces/' + enc(sibling.id), ['ready'], ['failed']);
        Object.assign(result, {
          siblingWorkspaceId: sibling.id,
          siblingWorkspaceStatus: readySibling.status,
        });
      }
    }
    if (name === 'workspace-git')
      Object.assign(result, {
        repositoryUrl: config.repository.url,
        ref: config.repository.ref,
        commit: config.repository.commit,
      });
    return result;
  }
  return factories;
}

export const supportedSetupNames = Object.freeze([...supported]);
