import { describe, expect, it } from 'vitest';

import { groupThinkingMessages } from './thinkingMessages';
import type { Message } from './types';

const first: Message = {
  id: 'thinking-1',
  sessionId: 'session-1',
  role: 'reasoning',
  content: 'Inspect the entry point.',
  reasoning: { status: 'completed' },
  createdAt: '2026-10-21T10:00:00Z',
  attachments: [],
  sequence: 1,
};
const second: Message = {
  ...first,
  id: 'thinking-2',
  content: 'Check the callers.',
  reasoning: { status: 'running' },
  sequence: 2,
};

describe('thinking display groups', () => {
  it('keeps the first identity through streamed updates without changing source records', () => {
    const source = [first, second];
    const [initial] = groupThinkingMessages(source);
    const updated = { ...second, content: `${second.content}\nThen verify the result.` };
    const [streamed] = groupThinkingMessages([first, updated]);

    expect(initial).toMatchObject({
      id: first.id,
      sequence: first.sequence,
      content: `${first.content}\n\n${second.content}`,
      reasoning: { status: 'running' },
    });
    expect(streamed).toMatchObject({
      id: first.id,
      content: `${first.content}\n\n${updated.content}`,
    });
    expect(source).toEqual([first, second]);
    expect(source[0]?.content).toBe('Inspect the entry point.');
    expect(source[0]?.reasoning?.status).toBe('completed');
    expect(source[1]?.content).toBe('Check the callers.');
    expect(groupThinkingMessages([first])[0]).toBe(first);
  });

  it.each([
    { label: 'sessions', next: { ...second, sessionId: 'session-2' } },
    { label: 'parent and worker', next: { ...second, agentId: 'worker-1' } },
    {
      label: 'workers',
      previous: { ...first, agentId: 'worker-1' },
      next: { ...second, agentId: 'worker-2' },
    },
  ])('never combines thinking from different $label', ({ previous = first, next }) => {
    expect(groupThinkingMessages([previous, next])).toEqual([previous, next]);
  });
});
