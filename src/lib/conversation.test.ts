import { describe, expect, it } from 'vitest';

import { emptyConversation, mergeMessages, receiveMessages, reduceEvent } from './conversation';
import type { Message, RuntimeEvent } from './types';

const message: Message = {
  attachments: [],
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
const reasoning: Message = {
  ...message,
  id: 'reasoning-1',
  role: 'reasoning',
  content: 'Inspecting',
  reasoning: { status: 'running' },
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

describe('thinking timeline reconciliation', () => {
  it('replaces streamed thinking in place without mixing it into the assistant answer', () => {
    const updated = { ...reasoning, content: 'Inspecting the project files' };
    const state = reduceEvent(
      { ...emptyConversation, status: 'running', messages: [reasoning], stream: 'The answer' },
      { sessionId: message.sessionId, kind: 'message', data: updated },
    );
    expect(state.messages).toEqual([updated]);
    expect(state.stream).toBe('The answer');
  });

  it.each([
    { kind: 'status', data: { status: 'idle' } },
    { kind: 'error', data: { message: 'Connection lost' } },
  ] satisfies Omit<RuntimeEvent, 'sessionId'>[])(
    'stops unfinished thinking after a $kind event while preserving its text',
    (event) => {
      const completed: Message = {
        ...reasoning,
        id: 'reasoning-completed',
        reasoning: { status: 'completed' },
      };
      const state = reduceEvent(
        { ...emptyConversation, status: 'running', messages: [completed, reasoning] },
        { ...event, sessionId: message.sessionId } as RuntimeEvent,
      );
      expect(state.messages[0]).toBe(completed);
      expect(state.messages[1]?.reasoning?.status).toBe('interrupted');
      expect(state.messages[1]?.content).toBe(reasoning.content);
    },
  );

  it('keeps live thinking when an overlapping history request returns an earlier chunk', () => {
    const live = { ...reasoning, content: 'Inspecting the project files' };
    const restored = receiveMessages(
      { ...emptyConversation, status: 'running', messages: [live] },
      [reasoning],
      new Set([reasoning.id]),
    );
    expect(restored.messages).toEqual([live]);
  });
});

describe('live message updates in ordered history', () => {
  it('replaces live thinking without changing historical identities or the previous list', () => {
    const history = Array.from({ length: 250 }, (_, index) => ({
      ...message,
      id: `answer-${String(index)}`,
      sequence: index,
    }));
    const live = { ...reasoning, sequence: history.length };
    const existing = mergeMessages([], [...history, live]);
    const updated = { ...live, content: 'Inspecting the complete dependency graph' };
    const next = mergeMessages(existing, [updated]);

    expect(next).toEqual([...history, updated]);
    expect(next.slice(0, history.length).every((item, index) => item === existing[index])).toBe(
      true,
    );
    expect(next.at(-1)).toBe(updated);
    expect(existing.at(-1)).toBe(live);
  });

  it('keeps a protected live update when stale history returns a single earlier chunk', () => {
    const live = { ...reasoning, content: 'Inspecting the complete dependency graph' };
    const existing = mergeMessages([], [live]);
    const next = mergeMessages(existing, [reasoning], new Set([live.id]));

    expect(next).toBe(existing);
    expect(next[0]).toBe(live);
  });

  it.each([
    {
      name: 'persisted sequence changes',
      first: { ...message, sequence: 1 },
      live: { ...reasoning, sequence: 2 },
      update: { ...reasoning, sequence: 0 },
    },
    {
      name: 'unsequenced creation time changes',
      first: message,
      live: { ...reasoning, createdAt: '2026-10-17T00:00:01Z' },
      update: { ...reasoning, createdAt: '2026-10-16T23:59:59Z' },
    },
  ])('restores chronological order when $name', ({ first, live, update }) => {
    const existing = mergeMessages([], [first, live]);
    expect(mergeMessages(existing, [update])).toEqual([update, first]);
    expect(existing).toEqual([first, live]);
  });

  it('still sorts an unreconciled list when a live update keeps its ordering keys', () => {
    const first = { ...message, sequence: 1 };
    const live = { ...reasoning, sequence: 2 };
    const updated = { ...live, content: 'Updated thinking' };
    expect(mergeMessages([live, first], [updated])).toEqual([first, updated]);
  });

  it('removes a matching pending send when its confirmation was already reconciled', () => {
    const confirmed: Message = { ...message, role: 'user' };
    const pending = { ...confirmed, id: `pending:${confirmed.id}` };
    const existing = mergeMessages([], [confirmed, pending]);
    expect(mergeMessages(existing, [confirmed])).toEqual([confirmed]);
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
