import { afterEach, expect, it, vi } from 'vitest';
import { attachmentMessage, type DraftAttachment, type UploadedAttachment } from './attachments';
import {
  acceptDraftCommand,
  agentDraftKey,
  agentDraftPrefix,
  clearOwnerDrafts,
  draftTextLimit,
  readAgentDraft,
  saveAgentDraft,
  type DraftValue,
} from './draftStorage';

const owner = 'synthetic-owner';
const agent = 'agt_draft';
const value = (content = '  Draft 草稿\n\t🧪  '): DraftValue => ({
  revision: 'revision-a',
  content,
  attachments: [],
});
const receipt: UploadedAttachment = {
  id: `att_${'a'.repeat(64)}`,
  agentId: agent,
  name: 'notes.txt',
  path: `/home/attachments/${agent}/att_${'a'.repeat(64)}/notes.txt`,
  size: 3,
  mediaType: 'text/plain',
  sha256: 'b'.repeat(64),
};
const ready: DraftAttachment = {
  key: 'file-a',
  file: new File(['abc'], 'notes.txt', { type: 'text/plain' }),
  status: 'ready',
  uploaded: receipt,
};
afterEach(() => vi.restoreAllMocks());

it('uses origin, owner and Agent in a separate versioned namespace and preserves exact text', () => {
  saveAgentDraft(owner, agent, value());
  expect(agentDraftKey(owner, agent)).toBe(
    `${agentDraftPrefix}${JSON.stringify([location.origin, owner, agent])}`,
  );
  expect(readAgentDraft(owner, agent)).toMatchObject(value());
  expect(readAgentDraft('other-owner', agent).content).toBe('');
  expect(readAgentDraft(owner, 'agt_other').content).toBe('');
  expect(JSON.parse(sessionStorage.getItem(agentDraftKey(owner, agent))!)).toMatchObject({
    version: 1,
    owner,
    agentId: agent,
    revision: 'revision-a',
  });
});

it('recovers uploaded metadata without a File and produces the same manifest', () => {
  const draft = { ...value(), attachments: [ready] };
  saveAgentDraft(owner, agent, draft);
  const restored = readAgentDraft(owner, agent);
  expect(restored.attachments[0]?.file).toBeUndefined();
  expect(restored.attachments[0]?.uploaded).toEqual(receipt);
  expect(attachmentMessage(restored.content, restored.attachments)).toBe(
    attachmentMessage(draft.content, draft.attachments),
  );
  expect(sessionStorage.getItem(agentDraftKey(owner, agent))).not.toContain('"file":');
});

it.each(['queued', 'uploading', 'error'] as const)(
  'recovers %s files as reselection placeholders without binary data',
  (status) => {
    saveAgentDraft(owner, agent, {
      ...value(),
      attachments: [{ ...ready, status, uploaded: undefined }],
    });
    const item = readAgentDraft(owner, agent).attachments[0]!;
    expect(item).toMatchObject({ status: 'reselect', description: { name: 'notes.txt', size: 3 } });
    expect(item.file).toBeUndefined();
    expect(item.uploaded).toBeUndefined();
    expect(attachmentMessage('message', [item])).toBe('');
  },
);

it.each([
  { agentId: 'agt_other' },
  { path: '/home/attachments/../secret' },
  { path: `${receipt.path}/../other` },
  { id: 'invalid' },
  { sha256: 'bad' },
  { size: -1 },
  { size: 21 * 1024 * 1024 },
  { mediaType: 'text/plain; unsafe' },
])('requires reselection of an unsafe saved reference: %j', (change) => {
  saveAgentDraft(owner, agent, { ...value(), attachments: [ready] });
  const key = agentDraftKey(owner, agent);
  const raw = JSON.parse(sessionStorage.getItem(key)!);
  Object.assign(raw.attachments[0].uploaded, change);
  sessionStorage.setItem(key, JSON.stringify(raw));
  expect(readAgentDraft(owner, agent).attachments[0]?.status).toBe('reselect');
});

it.each(['{', 'null', '{"version":0}', '{"version":99}'])(
  'refuses corrupt or incompatible records without overwriting them: %s',
  (raw) => {
    sessionStorage.setItem(agentDraftKey(owner, agent), raw);
    expect(() => readAgentDraft(owner, agent)).toThrow();
    expect(sessionStorage.getItem(agentDraftKey(owner, agent))).toBe(raw);
  },
);

it('bounds attachment count and descriptions', () => {
  expect(() =>
    saveAgentDraft(owner, agent, {
      ...value(),
      attachments: Array.from({ length: 11 }, (_, index) => ({ ...ready, key: String(index) })),
    }),
  ).toThrow();
  expect(() =>
    saveAgentDraft(owner, agent, {
      ...value(),
      attachments: [{ ...ready, file: new File(['a'], 'x'.repeat(1025)) }],
    }),
  ).toThrow();
});

it('refuses a seventh draft without evicting any existing draft', () => {
  for (let index = 0; index < 6; index++)
    saveAgentDraft(owner, `agt_${index}`, value(String(index)));
  expect(() => saveAgentDraft(owner, agent, value())).toThrow(/not saved/);
  for (let index = 0; index < 6; index++)
    expect(readAgentDraft(owner, `agt_${index}`).content).toBe(String(index));
  saveAgentDraft(owner, 'agt_0', value('updated'));
  expect(readAgentDraft(owner, 'agt_0').content).toBe('updated');
});

it('bounds UTF-8 text at 256 KiB and the serialized namespace at 1 MiB', () => {
  expect(() =>
    saveAgentDraft(owner, agent, value('界'.repeat(Math.ceil(draftTextLimit / 3)))),
  ).toThrow(/not saved/);
  for (let index = 0; index < 3; index++)
    saveAgentDraft(owner, `agt_${index}`, value('x'.repeat(draftTextLimit)));
  expect(() => saveAgentDraft(owner, 'agt_3', value('x'.repeat(draftTextLimit)))).toThrow(
    /not saved/,
  );
  expect(readAgentDraft(owner, 'agt_0').content.length).toBe(draftTextLimit);
});

it('reports read denial, write denial and browser quota errors to its caller', () => {
  const read = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new DOMException('Denied', 'SecurityError');
  });
  expect(() => readAgentDraft(owner, agent)).toThrow();
  read.mockRestore();
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  expect(() => saveAgentDraft(owner, agent, value())).toThrow();
  write.mockRestore();
});

it('clears only the draft bound to the reliably accepted command and preserves a newer revision', () => {
  saveAgentDraft(owner, agent, { ...value(), commandKey: 'command-a' });
  acceptDraftCommand(owner, agent, 'command-b', sessionStorage);
  expect(readAgentDraft(owner, agent).content).toBe(value().content);
  saveAgentDraft(owner, agent, { ...value(), revision: 'revision-b' });
  acceptDraftCommand(owner, agent, 'command-a', sessionStorage);
  expect(readAgentDraft(owner, agent).revision).toBe('revision-b');
  saveAgentDraft(owner, agent, { ...value(), commandKey: 'command-c' });
  acceptDraftCommand(owner, agent, 'command-c', sessionStorage);
  expect(sessionStorage.getItem(agentDraftKey(owner, agent))).toBeNull();
});

it('clears only the revoked owner and leaves creation drafts alone', () => {
  saveAgentDraft(owner, agent, value());
  saveAgentDraft('other-owner', agent, value());
  sessionStorage.setItem('cma:task-draft:synthetic', 'creation');
  clearOwnerDrafts(owner);
  expect(readAgentDraft(owner, agent).content).toBe('');
  expect(readAgentDraft('other-owner', agent).content).toBe(value().content);
  expect(sessionStorage.getItem('cma:task-draft:synthetic')).toBe('creation');
});
