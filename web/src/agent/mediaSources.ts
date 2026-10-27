import { createContext } from 'react';
import type { AgentFilesClient } from '../api/agentFiles';

export interface MessageMediaAccess {
  readonly agentId: string;
  readonly files: AgentFilesClient;
  readonly workingDirectory?: string;
}

export const MessageMediaContext = createContext<MessageMediaAccess | undefined>(undefined);

export function safeHttpUrl(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  try {
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password
      ? value
      : undefined;
  } catch {
    return undefined;
  }
}

function invalidPathCharacters(value: string) {
  return Array.from(value).some((character) => character.charCodeAt(0) < 32 || character === '\\');
}

/** Resolve only paths that the public Agent file API can authorize under /home/. */
export function messageFilePath(value: string | undefined, directory?: string): string | undefined {
  if (!value || value.startsWith('//') || invalidPathCharacters(value)) return undefined;
  let path = value;
  if (path.startsWith('sandbox:')) path = path.slice('sandbox:'.length);
  else if (/^[a-z][a-z\d+.-]*:/i.test(path)) return undefined;
  try {
    path = decodeURIComponent(path);
  } catch {
    return undefined;
  }
  if (invalidPathCharacters(path) || /[?#]/.test(path) || path.startsWith('//')) return undefined;
  if (!path.startsWith('/')) {
    if (!directory?.startsWith('/home/')) return undefined;
    path = `${directory}/${path}`;
  }
  if (!path.startsWith('/home/')) return undefined;
  const segments: string[] = [];
  for (const segment of path.slice('/home/'.length).split('/')) {
    if (segment === '..') {
      if (segments.length === 0) return undefined;
      segments.pop();
    } else if (segment !== '' && segment !== '.') segments.push(segment);
  }
  return segments.length > 0 ? segments.join('/') : undefined;
}

const mediaTypes: Readonly<Record<string, string>> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  gif: 'image/gif',
  avif: 'image/avif',
  bmp: 'image/bmp',
  mp3: 'audio/mpeg',
  wav: 'audio/wav',
  ogg: 'audio/ogg',
  opus: 'audio/ogg',
  m4a: 'audio/mp4',
  flac: 'audio/flac',
  mp4: 'video/mp4',
  webm: 'video/webm',
  mov: 'video/quicktime',
  m4v: 'video/mp4',
  ogv: 'video/ogg',
};

export function messageMediaType(value: string): string | undefined {
  let path = value;
  if (safeHttpUrl(value)) path = new URL(value).pathname;
  return mediaTypes[path.split('.').at(-1)?.toLowerCase() ?? ''];
}
