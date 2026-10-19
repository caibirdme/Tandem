import type { Message, Permission, RuntimeEvent, SessionStatus, ToolActivity } from './types';

export interface Conversation {
  messages: Message[];
  stream: string;
  tools: ToolActivity[];
  permissions: Permission[];
  status: SessionStatus;
  error: string | null;
  failedSend: Message | null;
}
export const emptyConversation: Conversation = {
  messages: [],
  stream: '',
  tools: [],
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

export function mergeMessages(existing: Message[], incoming: Message[]): Message[] {
  const messages = new Map(existing.map((message) => [message.id, message]));
  for (const message of incoming) {
    const pendingId = `pending:${message.id}`;
    if (confirmsPending(messages.get(pendingId), message)) messages.delete(pendingId);
    messages.set(message.id, message);
  }
  return [...messages.values()].toSorted((a, b) => a.createdAt.localeCompare(b.createdAt));
}

/** Live events and history confirm only the send carrying the same client-generated ID. */
export function receiveMessages(state: Conversation, messages: Message[]): Conversation {
  return {
    ...state,
    messages: mergeMessages(state.messages, messages),
    failedSend: messages.some((message) => confirmsPending(state.failedSend, message))
      ? null
      : state.failedSend,
  };
}

function applyStatus(state: Conversation, status: SessionStatus): Conversation {
  return {
    ...state,
    status,
    tools:
      status === 'running'
        ? state.tools
        : state.tools.map((tool) =>
            tool.status === 'running'
              ? { ...tool, status: status === 'error' ? 'error' : 'completed' }
              : tool,
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
    case 'tool': {
      const index = state.tools.findLastIndex((tool) =>
        event.data.toolCallId
          ? tool.toolCallId === event.data.toolCallId
          : tool.name === event.data.name && tool.status === 'running',
      );
      const previous = state.tools[index];
      const activity = {
        ...event.data,
        name: event.data.name === 'tool' && previous ? previous.name : event.data.name,
      };
      return {
        ...state,
        tools:
          index < 0
            ? [...state.tools, activity]
            : state.tools.map((tool, i) => (i === index ? activity : tool)),
      };
    }
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
