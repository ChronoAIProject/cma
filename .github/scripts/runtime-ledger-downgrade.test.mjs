import test from 'node:test';
import assert from 'node:assert/strict';
import { verifyRefusal } from './runtime-ledger-downgrade.mjs';
const state = {Running:false, ExitCode:2, OOMKilled:false};
const detail = 'runtime supervisor stopped: installation state directory contains an unknown file';
test('downgrade proof requires the exact early refusal and no serving', () => {
  verifyRefusal(state, detail);
  for (const changed of [{Running:true}, {ExitCode:0}, {OOMKilled:true}]) assert.throws(() => verifyRefusal({...state, ...changed}, detail));
  assert.throws(() => verifyRefusal(state, 'desktop failed'));
  assert.throws(() => verifyRefusal(state, detail + '\nruntime supervisor listening'));
});
