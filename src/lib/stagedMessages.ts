import type { SkillDraft } from './skillMentions';
import type { AttachmentInput } from './types';

export interface StagedMessage {
  id: string;
  draft: SkillDraft;
  attachments: AttachmentInput[];
  status: 'ready' | 'sending' | 'uncertain';
  error?: string | undefined;
}

/** Drafts stay on this device, scoped to a session, until explicitly submitted. */
export function createStagedMessages() {
  let state: ReadonlyMap<string, readonly StagedMessage[]> = new Map();
  const listeners = new Set<() => void>();
  function set(sessionId: string, items: readonly StagedMessage[]) {
    const next = new Map(state);
    if (items.length) next.set(sessionId, items);
    else next.delete(sessionId);
    state = next;
    for (const listener of listeners) listener();
  }
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    save(
      sessionId: string,
      draft: SkillDraft,
      attachments: AttachmentInput[],
      id: string = crypto.randomUUID(),
    ) {
      const items = state.get(sessionId) ?? [];
      const current = items.find((item) => item.id === id);
      if (current && current.status !== 'ready') return;
      const item: StagedMessage = { id, draft, attachments, status: 'ready' };
      set(
        sessionId,
        current ? items.map((entry) => (entry.id === id ? item : entry)) : [...items, item],
      );
    },
    remove(sessionId: string, id: string) {
      set(
        sessionId,
        (state.get(sessionId) ?? []).filter((item) => item.id !== id),
      );
    },
    begin(sessionId: string, id: string): StagedMessage | undefined {
      const items = state.get(sessionId) ?? [];
      const item = items.find((entry) => entry.id === id);
      if (!item || item.status !== 'ready' || items.some((entry) => entry.status === 'sending'))
        return;
      set(
        sessionId,
        items.map((entry) =>
          entry.id === id ? { ...entry, status: 'sending', error: undefined } : entry,
        ),
      );
      return item;
    },
    fail(sessionId: string, id: string, error: string, uncertain: boolean) {
      set(
        sessionId,
        (state.get(sessionId) ?? []).map((item) =>
          item.id === id ? { ...item, status: uncertain ? 'uncertain' : 'ready', error } : item,
        ),
      );
    },
    retain(sessionIds: ReadonlySet<string>) {
      for (const id of state.keys()) if (!sessionIds.has(id)) set(id, []);
    },
  };
}
export type StagedMessages = ReturnType<typeof createStagedMessages>;
