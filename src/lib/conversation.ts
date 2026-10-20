import type { Message, Permission, RuntimeEvent, SessionStatus } from './types';

export interface Conversation {
  messages: Message[];
  stream: string;
  permissions: Permission[];
  status: SessionStatus;
  error: string | null;
  failedSend: Message | null;
}
export const emptyConversation: Conversation = {
  messages: [],
  stream: '',
  permissions: [],
  status: 'idle',
  error: null,
  failedSend: null,
};

function confirmsPending(pending: Message | null | undefined, message: Message): boolean {
  return (
    message.role === 'user' &&
    pending?.sessionId === message.sessionId &&
    pending.id === `pending:${message.id}`
  );
}

export function mergeMessages(
  existing: Message[],
  incoming: Message[],
  protectedIds?: ReadonlySet<string>,
): Message[] {
  const messages = new Map(existing.map((message) => [message.id, message]));
  for (const message of incoming) {
    const pendingId = `pending:${message.id}`;
    if (confirmsPending(messages.get(pendingId), message)) messages.delete(pendingId);
    if (!protectedIds?.has(message.id) || !messages.has(message.id))
      messages.set(message.id, message);
  }
  return [...messages.values()].toSorted((a, b) => {
    if (a.sequence !== undefined && b.sequence !== undefined) return a.sequence - b.sequence;
    if (a.sequence !== undefined) return -1;
    if (b.sequence !== undefined) return 1;
    return a.createdAt.localeCompare(b.createdAt);
  });
}

/** Live events and history confirm only the send carrying the same client-generated ID. */
export function receiveMessages(
  state: Conversation,
  messages: Message[],
  protectedIds?: ReadonlySet<string>,
): Conversation {
  return {
    ...state,
    messages: mergeMessages(state.messages, messages, protectedIds),
    failedSend: messages.some((message) => confirmsPending(state.failedSend, message))
      ? null
      : state.failedSend,
  };
}

function applyStatus(state: Conversation, status: SessionStatus): Conversation {
  return {
    ...state,
    status,
    messages:
      status === 'running'
        ? state.messages
        : state.messages.map((message) =>
            message.activity?.status === 'running'
              ? {
                  ...message,
                  activity: {
                    status: message.activity.tools.some((tool) => tool.status === 'error')
                      ? 'error'
                      : 'interrupted',
                    tools: message.activity.tools.map((tool) =>
                      tool.status === 'running' ? { ...tool, status: 'interrupted' } : tool,
                    ),
                  },
                }
              : message,
          ),
    permissions: status === 'running' ? state.permissions : [],
    error: status === 'running' ? null : state.error,
  };
}

export function reduceEvent(state: Conversation, event: RuntimeEvent): Conversation {
  switch (event.kind) {
    case 'delta':
      return { ...state, stream: state.stream + event.data.content, status: 'running' };
    case 'message':
      return {
        ...receiveMessages(state, [event.data]),
        stream: event.data.role === 'assistant' ? '' : state.stream,
      };
    case 'permission':
      return {
        ...state,
        permissions: [
          ...state.permissions.filter((item) => item.requestId !== event.data.requestId),
          event.data,
        ],
      };
    case 'status':
      return applyStatus(state, event.data.status);
    case 'error':
      return { ...applyStatus(state, 'error'), error: event.data.message };
    case 'title':
    case 'title_error':
    case 'context_usage':
      return state;
  }
}
