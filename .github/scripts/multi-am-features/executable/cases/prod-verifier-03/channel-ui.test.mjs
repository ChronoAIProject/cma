import test from 'node:test';
import assert from 'node:assert/strict';
import {channelUiCases} from './channel-ui.mjs';

const byId = id => channelUiCases.find(definition => definition.id === `C275:${id}`);

test('EG-022 and EG-031 retain their frozen UI journeys as EG-only evidence', () => {
  for (const [id, fixtures] of [['EG-022', ['browser', 'user-fixtures', 'eg_browser']],
    ['EG-031', ['browser', 'user-fixtures', 'eg_browser']]]) {
    const definition = byId(id);
    assert.ok(definition, `${id} definition`);
    assert.equal(definition.evidence_scope, 'eg_only');
    assert.deepEqual(definition.fixtures, fixtures);
    assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  }
});

test('channel UI definitions keep the real operation-specific methods distinct', () => {
  const edit = byId('EG-022'), deletion = byId('EG-031');
  assert.notEqual(edit.run, deletion.run);
  assert.match(edit.id, /^C275:EG-022$/);
  assert.match(deletion.id, /^C275:EG-031$/);
});
