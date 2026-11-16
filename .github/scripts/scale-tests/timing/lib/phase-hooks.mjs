const captures = new WeakMap();

export function bindCapture(page, capture) { captures.set(page, capture); }
export function mark(page, label) { captures.get(page)?.mark(label); }
export function observedApi(page, method, path, action) {
  const capture = captures.get(page);
  return capture ? capture.apiRequest(method, path, action) : action();
}
