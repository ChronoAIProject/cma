import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import type { APIRequestContext } from '@playwright/test';
import { identifier, object } from './environment';

export type ResourceKind =
  | 'launches'
  | 'sandboxes'
  | 'workspaces'
  | 'agents'
  | 'agent-profiles'
  | 'workspace-profiles'
  | 'sandbox-profiles'
  | 'triggers';
export interface OwnedResource {
  kind: ResourceKind;
  id: string;
  subject: string;
}
export function launchIsQuiescent(row: Record<string, unknown>): boolean {
  if (['ready', 'completed'].includes(String(row.state))) return true;
  if (!['cancelled', 'failed_terminal'].includes(String(row.state))) return false;
  if (row.started_at === undefined) return true;
  const resolution = object(row.resolution);
  // Terminal cancellation/failure can follow an ambiguous effect; partial resolution is unsafe.
  return ['sandbox', 'workspace', 'agent'].every((kind) =>
    ['created', 'reused'].includes(String(object(resolution[kind]).state)),
  );
}
export class Journal {
  readonly resources: OwnedResource[] = [];
  private readonly seen = new Set<string>();
  constructor(
    private readonly output: string,
    private readonly run: string,
  ) {}
  record(row: Record<string, unknown>) {
    appendFileSync(
      join(this.output, 'cleanup.ndjson'),
      `${JSON.stringify({ run: this.run, at: new Date().toISOString(), ...row })}\n`,
      { mode: 0o600 },
    );
  }
  own(kind: ResourceKind, id: unknown, subject: string) {
    const resource = { kind, id: identifier(id, 'created resource ID'), subject };
    const key = `${kind}:${resource.id}`;
    if (this.seen.has(key)) return;
    this.seen.add(key);
    this.resources.push(resource);
    this.record({ action: 'created', ...resource });
  }
  observe(path: string, method: string, status: number, body: unknown, subject: string) {
    if (status < 200 || status >= 300 || body === null || typeof body !== 'object') return;
    const row = object(body);
    if (/^\/api\/v1\/triggers\/public\/[^/]+\/launches$/.test(path) && method === 'POST')
      this.own('launches', row.id, subject);
    const direct = path.match(
      /^\/api\/v1\/(sandboxes|agent-profiles|workspace-profiles|sandbox-profiles|triggers)$/,
    );
    if (method === 'POST' && status === 201 && direct)
      this.own(direct[1] as ResourceKind, row.id, subject);
    const child = path.match(/^\/api\/v1\/(?:sandboxes|workspaces)\/[^/]+\/(workspaces|agents)$/);
    if (method === 'POST' && status === 201 && child)
      this.own(child[1] as ResourceKind, row.id, subject);
    const fork = path.match(
      /^\/api\/v1\/marketplace\/(agent|workspace|sandbox)-profiles\/[^/]+\/fork$/,
    );
    if (method === 'POST' && status === 201 && fork)
      this.own(`${fork[1]}-profiles` as ResourceKind, row.id, subject);
    // Only resolutions belonging to a Launch admitted by this run may extend the cleanup set.
    if (typeof row.id !== 'string' || !this.seen.has(`launches:${row.id}`) || !row.resolution)
      return;
    const resolution = object(row.resolution);
    for (const [level, kind] of [
      ['sandbox', 'sandboxes'],
      ['workspace', 'workspaces'],
      ['agent', 'agents'],
    ] as const) {
      const value = object(resolution[level]);
      if (value.state === 'created') this.own(kind, value.id, subject);
    }
  }
  async cleanup(api: APIRequestContext, baseURL: string, subject: string) {
    if (!this.resources.some((row) => row.subject === subject)) return;
    const identity = await api.get(`${baseURL}/auth/session`);
    if (!identity.ok() || object(await identity.json()).subject !== subject) {
      this.record({ action: 'cleanup_refused', reason: 'identity_mismatch', subject });
      throw new Error(
        'Cleanup refused because the live identity no longer matches the resource owner',
      );
    }
    const owned = this.resources.filter((row) => row.subject === subject);
    let failed = false;
    // A 409 can mean already ready or a racing worker. Keep harvesting before any child cleanup.
    for (const resource of owned.filter((row) => row.kind === 'launches')) {
      const response = await api.post(`${baseURL}/api/v1/launches/${resource.id}/cancel`, {
        headers: { 'Idempotency-Key': crypto.randomUUID(), Origin: baseURL },
      });
      this.record({ action: 'cancel_requested', ...resource, status: response.status() });
      let quiescent = false;
      const deadline = Date.now() + 90_000;
      while (Date.now() < deadline) {
        const current = await api.get(`${baseURL}/api/v1/launches/${resource.id}`);
        if (current.ok()) {
          const row = object(await current.json());
          this.observe(`/api/v1/launches/${resource.id}`, 'GET', 200, row, subject);
          this.record({
            action: 'launch_observed',
            ...resource,
            state: row.state,
            revision: row.revision,
          });
          if (launchIsQuiescent(row)) {
            quiescent = true;
            break;
          }
          if (['cancelled', 'failed_terminal'].includes(String(row.state))) break;
        }
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
      if (!quiescent) {
        this.record({
          action: 'cleanup_blocked',
          ...resource,
          reason: 'launch_quiescence_unproved',
        });
        throw new Error(
          'Cleanup cannot prove Launch quiescence; retain every recorded resource for scoped operator reconciliation',
        );
      }
    }
    const order: ResourceKind[] = [
      'agents',
      'workspaces',
      'sandboxes',
      'triggers',
      'sandbox-profiles',
      'workspace-profiles',
      'agent-profiles',
    ];
    for (const kind of order) {
      for (const resource of this.resources
        .filter((row) => row.subject === subject && row.kind === kind)
        .reverse()) {
        const path = `/api/v1/${kind}/${resource.id}`;
        const read = await api.get(`${baseURL}${path}`);
        if (read.status() === 404) {
          this.record({ action: 'already_absent', ...resource });
          continue;
        }
        if (!read.ok()) {
          failed = true;
          this.record({ action: 'cleanup_read_failed', ...resource, status: read.status() });
          continue;
        }
        const row = object(await read.json());
        if (row.owner !== undefined && row.owner !== subject)
          throw new Error('Cleanup resource owner mismatch');
        if (kind === 'agents' && !['stopped', 'failed', 'deleted'].includes(String(row.status))) {
          const stop = await api.post(`${baseURL}${path}/stop`, {
            data: {},
            headers: { 'Idempotency-Key': crypto.randomUUID(), Origin: baseURL },
          });
          this.record({ action: 'stop_requested', ...resource, status: stop.status() });
          if (!stop.ok()) {
            failed = true;
            continue;
          }
          let stopped = false;
          const deadline = Date.now() + 90_000;
          while (Date.now() < deadline) {
            const current = await api.get(`${baseURL}${path}`);
            if (
              current.status() === 404 ||
              (current.ok() &&
                ['stopped', 'failed', 'deleted'].includes(
                  String(object(await current.json()).status),
                ))
            ) {
              stopped = true;
              break;
            }
            await new Promise((resolve) => setTimeout(resolve, 1000));
          }
          if (!stopped) {
            failed = true;
            this.record({ action: 'stop_pending', ...resource });
            continue;
          }
        }
        const retire =
          kind.endsWith('-profiles') || (kind === 'triggers' && row.availability !== 'draft');
        const response = await api.fetch(`${baseURL}${path}${retire ? '/retire' : ''}`, {
          method: retire ? 'POST' : 'DELETE',
          headers: { 'Idempotency-Key': crypto.randomUUID(), Origin: baseURL },
        });
        this.record({
          action: response.ok() ? 'cleanup_accepted' : 'cleanup_failed',
          ...resource,
          status: response.status(),
        });
        if (!response.ok()) {
          failed = true;
          continue;
        }
        let complete = false;
        const deadline = Date.now() + 90_000;
        while (Date.now() < deadline) {
          const current = await api.get(`${baseURL}${path}`);
          if (current.status() === 404) {
            complete = true;
            break;
          }
          if (!current.ok()) break;
          const value = object(await current.json());
          if (
            value.retired_at ||
            value.availability === 'retired' ||
            ['retired', 'deleted', 'removed'].includes(String(value.status))
          ) {
            complete = true;
            break;
          }
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
        this.record({ action: complete ? 'cleanup_complete' : 'cleanup_pending', ...resource });
        if (!complete) failed = true;
      }
    }
    if (failed)
      throw new Error(
        'Scoped cleanup is incomplete; consult cleanup.ndjson and retain the fixture owner',
      );
  }
}
