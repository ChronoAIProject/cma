import * as fs from 'node:fs/promises';
import {join, basename} from 'node:path';
import {tmpdir} from 'node:os';
import {hash} from '../../scale-tests/lib/checks.mjs';

export const binding = Object.freeze({campaign_id: 'campaign_fixture',
  candidate_sha256: hash('candidate'), fixture_sha256: hash('fixture')});

export function fixture(effectId = 'effect_fixture', operation = 'response_input') {
  const content = {body: Buffer.from('{ "input": "synthetic fixture" }\n'), key: Buffer.from('fixture-operation-key')};
  const intent = {schema: 'autopause-effect-v1', ...binding, effect_id: effectId,
    subject_sha256: hash('synthetic owner'), operation, resource_id: 'agent_fixture',
    request_sha256: hash(content.body), key_sha256: hash(content.key)};
  if (operation === 'settings_patch') {
    intent.resource_id = 'platform'; delete intent.key_sha256; delete content.key;
    content.original = Buffer.from('{ "enabled": false }');
    content.proposed = Buffer.from('{ "enabled": true }');
    Object.assign(intent, {expected_revision: '9007199254740993',
      original_values_sha256: hash(content.original), proposed_values_sha256: hash(content.proposed)});
  }
  const acknowledgement = operation === 'settings_patch'
    ? {resource_id: 'platform', revision: '9007199254740994', values_sha256: intent.proposed_values_sha256}
    : {agent_id: intent.resource_id, command_id: 'command_' + effectId, response_id: 'response_' + effectId};
  return {intent, content, ack: {type: 'acknowledged', source: 'direct_response', receipt: acknowledgement}};
}

export async function temporary(action) {
  const parent = await fs.realpath(await fs.mkdtemp(join(tmpdir(), 'autopause-custody-')));
  try {return await action(join(parent, 'journal'), parent);}
  finally {await fs.rm(parent, {recursive: true, force: true});}
}

export function deferred() {
  let resolve;
  const promise = new Promise(done => {resolve = done;});
  return {promise, resolve};
}

// Faults wrap real file operations. Hooks can withhold sync completion while a
// second reader observes visible bytes; no in-memory filesystem replaces disk.
export function observedIO(hook) {
  let phase = 'create';
  const invoke = async (point, action) => {
    await hook('before:' + point);
    const value = await action();
    await hook('after:' + point);
    return value;
  };
  return {...fs,
    mkdir: (...args) => invoke('mkdir', () => fs.mkdir(...args)),
    async open(path, ...args) {
      const file = await fs.open(path, ...args);
      const name = basename(path);
      const journal = name === 'custody.jsonl';
      const directory = (await file.stat()).isDirectory();
      const label = () => directory ? 'directory:' + phase : journal ? 'journal:' + phase : 'content:' + name.split('.').at(-1);
      return {
        stat: (...values) => file.stat(...values), read: (...values) => file.read(...values),
        close: () => file.close(),
        async writeFile(data) {
          if (journal) {
            const payload = JSON.parse(data).payload;
            phase = payload.type === 'event' ? payload.event.type : payload.type;
          } else phase = 'content_' + name.split('.').at(-1);
          return invoke('write:' + label(), () => file.writeFile(data));
        },
        sync: () => invoke('sync:' + label(), () => file.sync()),
      };
    },
  };
}
