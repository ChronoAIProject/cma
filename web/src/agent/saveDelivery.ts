export function saveDelivery(bytes: Uint8Array, name: string) {
  saveAgentDownload(new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' }), name);
}

export function saveAgentDownload(blob: Blob, name: string) {
  const link = document.createElement('a');
  const url = URL.createObjectURL(blob);
  link.href = url;
  link.download = name;
  link.rel = 'noreferrer';
  try {
    document.body.append(link);
    link.click();
  } finally {
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
}
