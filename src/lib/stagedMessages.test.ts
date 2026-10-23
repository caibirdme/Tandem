import { describe, expect, it } from 'vitest';

import { createStagedMessages } from './stagedMessages';

const draft = { text: 'Use another approach', mentions: [] };
const attachments = [{ name: 'notes.txt', mimeType: 'text/plain', data: 'aGk=' }];

describe('session-scoped staged messages', () => {
  it('edits in place and preserves independent sessions and complete attachments', () => {
    const store = createStagedMessages();
    store.save('one', draft, attachments, 'id');
    store.save('two', draft, [], 'other');
    store.save('one', { ...draft, text: 'Updated' }, attachments, 'id');
    expect(store.getSnapshot().get('one')).toEqual([
      { id: 'id', draft: { ...draft, text: 'Updated' }, attachments, status: 'ready' },
    ]);
    expect(store.getSnapshot().get('two')).toHaveLength(1);
    store.retain(new Set(['two']));
    expect(store.getSnapshot().has('one')).toBe(false);
  });
  it('admits only one in-flight submission per session and never retries uncertain delivery', () => {
    const store = createStagedMessages();
    store.save('one', draft, attachments, 'first');
    store.save('one', draft, [], 'second');
    expect(store.begin('one', 'first')).toBeDefined();
    expect(store.begin('one', 'first')).toBeUndefined();
    expect(store.begin('one', 'second')).toBeUndefined();
    store.fail('one', 'first', 'Connection lost', true);
    expect(store.begin('one', 'first')).toBeUndefined();
    store.save('one', draft, [], 'first');
    expect(store.getSnapshot().get('one')?.[0]?.status).toBe('uncertain');
    expect(store.begin('one', 'second')).toBeDefined();
  });
  it('retains the same message id for a definitive rejection and releases deleted payloads', () => {
    const store = createStagedMessages();
    store.save('one', draft, attachments, 'id');
    store.begin('one', 'id');
    store.fail('one', 'id', 'Invalid selection', false);
    expect(store.begin('one', 'id')?.id).toBe('id');
    store.retain(new Set());
    store.fail('one', 'id', 'Late completion', true);
    expect(store.getSnapshot().size).toBe(0);
  });
});
