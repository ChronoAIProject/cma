export async function readConfig(input) {
  const decoder = new TextDecoder('utf-8', {fatal: true});
  let text = '', bytes = 0;
  for await (const chunk of input) {
    bytes += chunk.byteLength;
    if (bytes > 64 * 1024) throw new Error('client_config_bound');
    text += decoder.decode(chunk, {stream: true});
  }
  return JSON.parse(text + decoder.decode());
}
