import type { AttachmentInput, Message, Permission, RuntimeEvent, SessionStatus } from './types';

export interface ConversationMessage extends Message {
  pendingAttachments?: AttachmentInput[];
}

export interface Conversation {
  messages: ConversationMessage[];
  stream: string;
  agentStreams: Record<string, string>;
  permissions: Permission[];
  status: SessionStatus;
  error: string | null;
  failedSend: ConversationMessage | null;
}
export const emptyConversation: Conversation = {
  messages: [],
  stream: '',
  agentStreams: {},
  permissions: [],
  status: 'idle',
  error: null,
  failedSend: null,
};

// Only reconciled lists can skip sorting when a live update keeps its ordering keys.
const orderedMessageLists = new WeakSet<ConversationMessage[]>();

function confirmsPending(pending: Message | null | undefined, message: Message): boolean {
  return (
    message.role === 'user' &&
    pending?.sessionId === message.sessionId &&
    pending.id === `pending:${message.id}`
  );
}

export function mergeMessages(
  existing: ConversationMessage[],
  incoming: Message[],
  protectedIds?: ReadonlySet<string>,
): ConversationMessage[] {
  const update = incoming.length === 1 ? incoming[0] : undefined;
  if (update && update.role !== 'user' && orderedMessageLists.has(existing)) {
    const index = existing.findIndex((message) => message.id === update.id);
    const previous = existing[index];
    if (
      previous &&
      previous.sequence === update.sequence &&
      previous.createdAt === update.createdAt
    ) {
      if (protectedIds?.has(update.id) || previous === update) return existing;
      const messages = existing.with(index, update);
      orderedMessageLists.add(messages);
      return messages;
    }
  }

  const messages = new Map(existing.map((message) => [message.id, message]));
  for (const message of incoming) {
    const pendingId = `pending:${message.id}`;
    if (confirmsPending(messages.get(pendingId), message)) messages.delete(pendingId);
    if (!protectedIds?.has(message.id) || !messages.has(message.id))
      messages.set(message.id, message);
  }
  const ordered = [...messages.values()].toSorted((a, b) => {
    if (a.sequence !== undefined && b.sequence !== undefined) return a.sequence - b.sequence;
    if (a.sequence !== undefined) return -1;
    if (b.sequence !== undefined) return 1;
    return a.createdAt.localeCompare(b.createdAt);
  });
  orderedMessageLists.add(ordered);
  return ordered;
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
    agentStreams: status === 'running' ? state.agentStreams : {},
    messages:
      status === 'running'
        ? state.messages
        : state.messages.map((message) =>
            message.agent?.status === 'running'
              ? { ...message, agent: { ...message.agent, status: 'interrupted' } }
              : message.activity?.status === 'running'
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
                : message.reasoning?.status === 'running'
                  ? { ...message, reasoning: { status: 'interrupted' } }
                  : message,
          ),
    permissions: status === 'running' ? state.permissions : [],
    error: status === 'running' ? null : state.error,
  };
}

export function reduceEvent(state: Conversation, event: RuntimeEvent): Conversation {
  switch (event.kind) {
    case 'delta':
      if (event.data.agentId)
        return {
          ...state,
          agentStreams: {
            ...state.agentStreams,
            [event.data.agentId]:
              (state.agentStreams[event.data.agentId] ?? '') + event.data.content,
          },
          status: 'running',
        };
      return { ...state, stream: state.stream + event.data.content, status: 'running' };
    case 'message': {
      const agentStreams = { ...state.agentStreams };
      if (event.data.role === 'assistant' && event.data.agentId)
        delete agentStreams[event.data.agentId];
      return {
        ...receiveMessages(state, [event.data]),
        agentStreams,
        stream: event.data.role === 'assistant' && !event.data.agentId ? '' : state.stream,
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
