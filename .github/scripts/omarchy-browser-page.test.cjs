const assert = require('node:assert/strict');
const test = require('node:test');
const vm = require('node:vm');
const page = require('./omarchy-browser-page.cjs');

function load(path, retained = '') {
  const html = page(path);
  const listeners = {};
  const element = name => ({ value: '', addEventListener: (event, handler) => { listeners[name + ':' + event] = handler; } });
  const field = element('field'), verify = element('verify');
  const document = { title: html.match(/<title>(.*?)<\/title>/)[1], getElementById: id => ({ field, verify })[id] };
  assert.equal(document.title, 'cma-desktop-loading');
  const window = { innerWidth: 1256, innerHeight: 670,
    addEventListener: (event, handler) => { listeners['window:' + event] = handler; } };
  const storage = new Map([['cma-desktop-proof', retained]]), fetches = [];
  vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], {
    document, window, fetch: path => { fetches.push(path); },
    localStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) },
  });
  return { document, window, field, storage, fetches, fire: event => listeners[event]() };
}

test('initial launcher load cannot acknowledge the keyboard navigation', () => {
  const initial = load('/desktop-test');
  assert.equal(initial.document.title, 'cma-desktop-ready');
  Object.assign(initial.window, { innerWidth: 1280, innerHeight: 800 });
  initial.fire('window:resize');
  assert.equal(initial.document.title, 'cma-desktop-ready');
  assert.equal(load('/desktop-input').document.title, 'cma-desktop-input-ready');
});

test('browser chrome or scaled viewport cannot acknowledge fullscreen', () => {
  const p = load('/desktop-input');
  for (const [width, height] of [[1280, 720], [1256, 800], [640, 400]]) {
    Object.assign(p.window, { innerWidth: width, innerHeight: height });
    p.fire('window:resize');
    assert.equal(p.document.title, 'cma-desktop-input-ready');
  }
  Object.assign(p.window, { innerWidth: 1280, innerHeight: 800 });
  p.fire('window:resize');
  assert.equal(p.document.title, 'cma-desktop-fullscreen');
});

test('focus and typing cannot forge button completion or profile persistence', () => {
  const p = load('/desktop-input', 'cma-user');
  assert.deepEqual(p.fetches, ['/profile?value=cma-user']);
  p.fire('field:focus');
  assert.equal(p.document.title, 'cma-input-focused');
  p.field.value = 'cma-codex';
  p.fire('field:input');
  assert.equal(p.document.title, 'cma-input:cma-codex');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-user');
  p.fire('verify:click');
  assert.equal(p.document.title, 'cma-ui:cma-codex');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-codex');
});

test('the later user still controls the same page and retained profile', () => {
  const p = load('/desktop-input');
  p.field.value = 'cma-codex'; p.fire('verify:click');
  p.fire('field:focus'); p.field.value = 'cma-user'; p.fire('field:input');
  p.fire('verify:click');
  Object.assign(p.window, { innerWidth: 1280, innerHeight: 800 }); p.fire('window:resize');
  assert.equal(p.document.title, 'cma-ui:cma-user');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-user');
});
