const assert = require('node:assert/strict');
const test = require('node:test');
const vm = require('node:vm');
const page = require('./desktop-browser-page.cjs');

function load(path, { retained = '', width = 1040, height = 640 } = {}) {
  const html = page(path), listeners = {};
  const element = name => ({ value: '', addEventListener: (event, fn) => { listeners[name + ':' + event] = fn; } });
  const field = element('field'), verify = element('verify');
  const document = { title: html.match(/<title>(.*?)<\/title>/)[1], getElementById: id => ({ field, verify })[id] };
  assert.equal(document.title, 'cma-desktop-loading');
  const window = { innerWidth: width, innerHeight: height,
    addEventListener: (event, fn) => { listeners['window:' + event] = fn; } };
  const storage = new Map([['cma-desktop-proof', retained]]), fetches = [];
  vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], {
    document, window, fetch: value => { fetches.push(value); },
    localStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) },
  });
  return { document, window, field, storage, fetches, fire: event => listeners[event]() };
}

test('initial page cannot acknowledge keyboard navigation, even after resize', () => {
  const p = load('/desktop-test');
  assert.equal(p.document.title, 'cma-desktop-ready|1040x640');
  Object.assign(p.window, { innerWidth: 1280, innerHeight: 800 }); p.fire('window:resize');
  assert.equal(p.document.title, 'cma-desktop-ready|1280x800');
  assert.equal(load('/desktop-input').document.title, 'cma-desktop-input-ready|1040x640');
});

test('viewport acknowledges actual dimensions, including restored fullscreen', () => {
  const p = load('/desktop-input', { width: 1280, height: 800 });
  assert.equal(p.document.title, 'cma-desktop-input-ready|1280x800');
  for (const [width, height] of [[1280, 720], [1256, 800], [640, 400]]) {
    Object.assign(p.window, { innerWidth: width, innerHeight: height }); p.fire('window:resize');
    assert.equal(p.document.title, `cma-desktop-input-ready|${width}x${height}`);
  }
});

test('focus and typing do not acknowledge button activation or save the profile', () => {
  const p = load('/desktop-input', { retained: 'cma-user', width: 1280, height: 800 });
  assert.deepEqual(p.fetches, ['/profile?value=cma-user']);
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-user');
  p.fire('field:focus'); assert.equal(p.document.title, 'cma-input-focused|1280x800');
  p.field.value = 'cma-codex'; p.fire('field:input');
  assert.equal(p.document.title, 'cma-input:cma-codex|1280x800');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-user');
  p.fire('verify:click');
  assert.equal(p.document.title, 'cma-ui:cma-codex|1280x800');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-codex');
});

test('manual user input and window normalization preserve the same page and profile', () => {
  const p = load('/desktop-input', { width: 1280, height: 800 });
  p.field.value = 'cma-codex'; p.fire('verify:click');
  p.fire('field:focus'); p.field.value = 'cma-user'; p.fire('field:input'); p.fire('verify:click');
  Object.assign(p.window, { innerWidth: 1040, innerHeight: 640 }); p.fire('window:resize');
  assert.equal(p.document.title, 'cma-ui:cma-user|1040x640');
  assert.equal(p.storage.get('cma-desktop-proof'), 'cma-user');
});
