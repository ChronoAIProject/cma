export function createAdapter(config) {
  async function call(method, body = {}) {
    const response = await fetch(`${config.control_origin}/${method}`, {
      method: 'POST',
      headers: {'Content-Type': 'application/json', Authorization: `Bearer ${config.control_token}`},
      body: JSON.stringify(body), signal: AbortSignal.timeout(180000),
    });
    const value = await response.json();
    if (!response.ok) throw new Error(`adapter ${method}: ${value.error || response.status}`);
    return value;
  }
  return Object.fromEntries(['topology','runtimeFor','prepareTurn','ownerOf','killAm',
    'releaseTurn','oracle','restoreAm','captureFailure'].map(name=>[name,(body)=>call(name,body)]));
}
export default createAdapter;
