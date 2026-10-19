import { describe, expect, it } from 'vitest';

import { emptyConversation, mergeMessages, reduceEvent } from './conversation';
import type { Message, RuntimeEvent } from './types';

const message: Message = {
  id: 'message-1',
  sessionId: 'session-1',
  role: 'assistant',
  content: 'Hello',
  createdAt: '2026-10-17T00:00:00Z',
};

describe('conversation stream reconciliation', () => {
  it.each([
    {
      name: 'replaces streaming content with persisted assistant response',
      events: [
        { kind: 'delta', data: { content: 'Hel' } },
        { kind: 'delta', data: { content: 'lo' } },
        { kind: 'message', data: message },
      ],
      stream: '',
      count: 1,
    },
    {
      name: 'does not duplicate replayed persisted messages',
      events: [
        { kind: 'message', data: message },
        { kind: 'message', data: message },
      ],
      stream: '',
      count: 1,
    },
    {
      name: 'preserves partial response when generation is interrupted',
      events: [
        { kind: 'delta', data: { content: 'Hello' } },
        { kind: 'status', data: { status: 'idle' } },
      ],
      stream: 'Hello',
      count: 0,
    },
  ] satisfies {
    name: string;
    events: Omit<RuntimeEvent, 'sessionId'>[];
    stream: string;
    count: number;
  }[])('$name', ({ events, stream, count }) => {
    const state = events.reduce(
      (current, event) =>
        reduceEvent(current, { ...event, sessionId: 'session-1' } as RuntimeEvent),
      emptyConversation,
    );
    expect(state.stream).toBe(stream);
    expect(state.messages).toHaveLength(count);
  });

  it('merges history without acknowledging an unrelated pending send', () => {
    const pending: Message = { ...message, role: 'user', id: 'pending:1' };
    const older: Message = {
      ...pending,
      id: 'older',
      content: 'Previous turn',
      createdAt: '2026-10-16T00:00:00Z',
    };
    expect(mergeMessages([pending, message], [older])).toEqual([older, pending, message]);
  });

  it('acknowledges the pending send even if history already includes the live user message', () => {
    const pending: Message = { ...message, role: 'user', id: 'pending:1' };
    const confirmed: Message = { ...pending, id: '1' };
    const state = reduceEvent(
      { ...emptyConversation, messages: [pending, confirmed, message] },
      { sessionId: message.sessionId, kind: 'message', data: confirmed },
    );
    expect(state.messages).toEqual([confirmed, message]);
  });

  it('clears unanswered approvals when a run ends', () => {
    const state = reduceEvent(
      {
        ...emptyConversation,
        permissions: [{ requestId: '1', kind: 'write', description: 'Edit file' }],
        tools: [{ name: 'create', status: 'running', content: '' }],
      },
      {
        sessionId: 'session-1',
        kind: 'error',
        data: { message: 'Disconnected' },
      },
    );
    expect(state.permissions).toEqual([]);
    expect(state.error).toBe('Disconnected');
    expect(state.tools[0]?.status).toBe('error');
  });
});

describe('tool activity correlation', () => {
  it.each([
    {
      name: 'preserves the tool name on generic completion',
      id: 'a',
      expected: ['completed', 'running'],
    },
    {
      name: 'updates the correct parallel call with the same name',
      id: 'b',
      expected: ['running', 'completed'],
    },
  ])('$name', ({ id, expected }) => {
    const state = reduceEvent(
      {
        ...emptyConversation,
        tools: [
          { toolCallId: 'a', name: 'create', status: 'running', content: 'first.txt' },
          { toolCallId: 'b', name: 'create', status: 'running', content: 'second.txt' },
        ],
      },
      {
        sessionId: 'session-1',
        kind: 'tool',
        data: { toolCallId: id, name: 'tool', status: 'completed', content: 'OK' },
      },
    );
    expect(state.tools.map((tool) => tool.status)).toEqual(expected);
    expect(state.tools.map((tool) => tool.name)).toEqual(['create', 'create']);
  });
});
