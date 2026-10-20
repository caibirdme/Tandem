import { describe, expect, it } from 'vitest';

import { emptyConversation, mergeMessages, receiveMessages, reduceEvent } from './conversation';
import type { Message, RuntimeEvent } from './types';

const message: Message = {
  id: 'message-1',
  sessionId: 'session-1',
  role: 'assistant',
  content: 'Hello',
  createdAt: '2026-10-17T00:00:00Z',
};
const activity: Message = {
  ...message,
  id: 'activity-1',
  role: 'activity',
  content: 'Inspecting the project',
  activity: {
    status: 'running',
    tools: [{ toolCallId: 'read-1', name: 'read_file', status: 'running' }],
  },
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
        messages: [activity],
      },
      {
        sessionId: 'session-1',
        kind: 'error',
        data: { message: 'Disconnected' },
      },
    );
    expect(state.permissions).toEqual([]);
    expect(state.error).toBe('Disconnected');
    expect(state.messages[0]?.activity).toMatchObject({
      status: 'interrupted',
      tools: [{ status: 'interrupted' }],
    });
  });
});

describe('activity timeline reconciliation', () => {
  it.each([
    {
      name: 'keeps a partial response when a new activity group arrives',
      messages: [],
    },
    {
      name: 'keeps an unrelated final answer when an activity group is updated',
      messages: [activity],
    },
  ])('$name', ({ messages }) => {
    const state = reduceEvent(
      { ...emptyConversation, messages, stream: 'Current response' },
      {
        sessionId: 'session-1',
        kind: 'message',
        data: activity,
      },
    );
    expect(state.stream).toBe('Current response');
    expect(state.messages).toEqual([activity]);
  });

  it('upserts a group in place while keeping tools with the same name distinct', () => {
    const completed: Message = {
      ...activity,
      activity: {
        status: 'completed',
        tools: [
          { toolCallId: 'read-1', name: 'read_file', status: 'completed', result: 'First file' },
          { toolCallId: 'read-2', name: 'read_file', status: 'completed', result: 'Second file' },
        ],
      },
    };
    const final: Message = {
      ...message,
      id: 'answer',
      content: 'Found the problem',
      createdAt: '2026-10-17T00:00:01Z',
    };
    const state = reduceEvent(
      { ...emptyConversation, messages: [activity, final] },
      { kind: 'message', sessionId: 'session-1', data: completed },
    );
    expect(state.messages).toEqual([completed, final]);
  });

  it('preserves persisted order across equal timestamps and a later turn', () => {
    const first = { ...activity, sequence: 2 };
    const final = { ...message, sequence: 3 };
    const nextUser: Message = { ...message, id: 'user-2', role: 'user', sequence: 4 };
    const nextGroup = { ...activity, id: 'activity-2', sequence: 5 };
    const restored = receiveMessages({ ...emptyConversation, messages: [nextUser, nextGroup] }, [
      final,
      first,
      nextGroup,
    ]);
    expect(restored.messages.map((item) => item.id)).toEqual([
      first.id,
      final.id,
      nextUser.id,
      nextGroup.id,
    ]);
  });

  it('keeps optimistic sends after persisted messages regardless of client clock skew', () => {
    const group = { ...activity, sequence: 2, createdAt: '2026-10-17T00:00:02Z' };
    const final = { ...message, sequence: 3, createdAt: '2026-10-17T00:00:00Z' };
    const pending: Message = {
      ...message,
      role: 'user',
      id: 'pending:2',
      createdAt: '2026-10-16T23:59:59Z',
    };
    for (const existing of [
      [pending, final, group],
      [group, pending, final],
    ]) {
      expect(mergeMessages(existing, []).map((item) => item.id)).toEqual([
        group.id,
        final.id,
        pending.id,
      ]);
    }
  });

  it('does not roll back a live group when an overlapping history request finishes', () => {
    const completed: Message = {
      ...activity,
      activity: { status: 'completed', tools: [] },
    };
    const earlierUser: Message = {
      ...message,
      id: 'user-1',
      role: 'user',
      createdAt: '2026-10-16T23:59:59Z',
    };
    const restored = receiveMessages(
      { ...emptyConversation, messages: [completed] },
      [earlierUser, activity],
      new Set([activity.id]),
    );
    expect(restored.messages).toEqual([earlierUser, completed]);
  });

  it('marks unfinished tools as stopped when a run ends without their completion', () => {
    const state = reduceEvent(
      { ...emptyConversation, messages: [activity] },
      { kind: 'status', sessionId: 'session-1', data: { status: 'idle' } },
    );
    expect(state.messages[0]?.activity).toMatchObject({
      status: 'interrupted',
      tools: [{ status: 'interrupted' }],
    });
  });

  it('retains explicit failures while distinguishing unfinished calls after a session error', () => {
    const state = reduceEvent(
      {
        ...emptyConversation,
        messages: [
          {
            ...activity,
            activity: {
              status: 'running',
              tools: [
                { toolCallId: 'failed-1', name: 'run_check', status: 'error' },
                { toolCallId: 'running-1', name: 'read_file', status: 'running' },
              ],
            },
          },
        ],
      },
      { kind: 'error', sessionId: 'session-1', data: { message: 'Connection lost' } },
    );
    expect(state.messages[0]?.activity).toMatchObject({
      status: 'error',
      tools: [{ status: 'error' }, { status: 'interrupted' }],
    });
  });
});
