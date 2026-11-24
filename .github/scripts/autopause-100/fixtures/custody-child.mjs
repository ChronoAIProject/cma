import {open} from 'node:fs/promises';
import {join, dirname} from 'node:path';
import {CustodyJournal} from '../custody-journal.mjs';
import {binding, fixture, observedIO} from './custody.mjs';

const [directory, target] = process.argv.slice(2);
async function stopAt(stage) {
  if (stage !== target) return;
  process.send({stage});
  await new Promise(resolve => {process.once('message', resolve);});
}
const io = observedIO(async point => {
  if (point === 'after:sync:directory:intent') await stopAt('registered');
  if (point === 'after:sync:directory:dispatch_recorded') await stopAt('dispatched');
  if (point === 'before:sync:journal:acknowledged') await stopAt('visible_ack');
});
const value = fixture();
const writer = await CustodyJournal.create(directory, binding, {io});
try {
  const ready = await writer.recordIntent(value.intent, value.content);
  await stopAt('ready');
  await writer.dispatch(value.intent.effect_id, ready, async () => {
    const counter = await open(join(dirname(directory), 'synthetic-counter'), 'a', 0o600);
    try {await counter.writeFile('1\n'); await counter.sync();} finally {await counter.close();}
    await stopAt('effect_called');
    return value.ack;
  });
  await stopAt('acknowledged');
} finally {await writer.close();}
