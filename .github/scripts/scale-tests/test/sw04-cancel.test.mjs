import assert from 'node:assert/strict';
import { test } from 'node:test';
import { hash, isHash } from '../lib/checks.mjs';
import { runSw04Fixture } from '../lib/sw04-cancel.mjs';

function observation() {
  const owner = { processHash: hash('owner'), epoch: 3 };
  const peer = hash('peer');
  const commandHash = hash('cancel');
  const responseHash = hash('active-response');
  const effect = { kind: 'interrupt', commandHash, processHash: owner.processHash,
    ownerProcessHash: owner.processHash, epoch: owner.epoch, runtimeGeneration: 2 };
  return {
    schema: 'cma-sw04-cancel-v1', replicas: [owner.processHash, peer], owner,
    admission: { status: 202, processHash: peer, commandHash, responseHash },
    replay: { status: 202, processHash: owner.processHash, commandHash, responseHash },
    durable: { commandHash, responseHash, status: 'completed', runtimeGeneration: 2 },
    effectsBeforeReplay: [{ ...effect }], effectsAfterReplay: [{ ...effect }],
  };
}

const frame = (value) => `SW04_CANCEL_OBSERVATION=${JSON.stringify(value)}`;

test('SW04 reconciles the process, receipt and runtime observations in the fixture log', () => {
  const value = observation();
  value.unexpected = 'private diagnostic';
  const report = runSw04Fixture(`running 1 test\ntest module::name ... ${frame(value)}\nok\n`);
  assert.equal(report.status, 'PASS');
  assert.equal(report.track_status, 'INCOMPLETE');
  const [result] = report.scenarios;
  assert.equal(result.scenario, 'sw04_cancel');
  assert.equal(result.evidence.commands, 1);
  assert.equal(result.evidence.effects, 1);
  assert.equal(result.evidence.additional_replay_effects, 0);
  assert(isHash(result.evidence_hash));
  assert(!JSON.stringify(report).includes(value.unexpected));
});

const failures = [
  ['duplicate process', (v) => v.replicas.fill(v.owner.processHash), 'sw04_replica_identity_missing', 'BLOCKED'],
  ['invalid epoch', (v) => v.owner.epoch = 0, 'sw04_owner_authority_missing', 'BLOCKED'],
  ['owner accepted initial request', (v) => v.admission.processHash = v.owner.processHash, 'sw04_wrong_ingress'],
  ['peer received replay', (v) => v.replay.processHash = v.admission.processHash, 'sw04_wrong_replay_ingress'],
  ['receipt changed', (v) => v.replay.commandHash = hash('other-command'), 'sw04_replay_identity_changed'],
  ['different response on replay', (v) => v.replay.responseHash = hash('other-response'), 'sw04_replay_identity_changed'],
  ['missing terminal outcome', (v) => v.durable.status = 'in_progress', 'sw04_durable_outcome_invalid'],
  ['receiver absent', (v) => delete v.effectsBeforeReplay, 'sw04_receiver_observation_missing', 'BLOCKED'],
  ['missing effect', (v) => v.effectsBeforeReplay.pop(), 'sw04_effect_missing'],
  ['duplicate before replay', (v) => v.effectsBeforeReplay.push(v.effectsBeforeReplay[0]), 'sw04_effect_duplicate'],
  ['effect replays', (v) => v.effectsAfterReplay.push(v.effectsAfterReplay[0]), 'sw04_effect_duplicate'],
  ['wrong receiving process', (v) => v.effectsAfterReplay[0].processHash = v.admission.processHash, 'sw04_effect_owner_invalid'],
  ['wrong owner authority', (v) => v.effectsBeforeReplay[0].ownerProcessHash = v.admission.processHash, 'sw04_effect_owner_invalid'],
  ['stale epoch', (v) => v.effectsBeforeReplay[0].epoch--, 'sw04_effect_owner_invalid'],
  ['different command received', (v) => v.effectsBeforeReplay[0].commandHash = hash('other-command'), 'sw04_effect_identity_invalid'],
  ['different runtime generation', (v) => v.effectsAfterReplay[0].runtimeGeneration++, 'sw04_effect_identity_invalid'],
];

for (const [name, change, code, status = 'FAIL'] of failures) {
  test(`SW04 rejects ${name}`, () => {
    const value = observation();
    change(value);
    assert.deepEqual(runSw04Fixture(frame(value)).scenarios, [{ scenario: 'sw04_cancel', status, code }]);
  });
}

test('SW04 refuses absent, malformed and repeated observation frames', () => {
  const cases = [
    ['', 'sw04_observation_missing', 'BLOCKED'],
    ['SW04_CANCEL_OBSERVATION=private diagnostic', 'sw04_observation_malformed', 'FAIL'],
    [`${frame(observation())}\n${frame(observation())}`, 'sw04_observation_duplicate', 'FAIL'],
  ];
  for (const [log, code, status] of cases) {
    assert.deepEqual(runSw04Fixture(log).scenarios, [{ scenario: 'sw04_cancel', status, code }]);
  }
});
