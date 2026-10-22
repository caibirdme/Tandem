import type { Conversation } from './conversation';

// Add future panel kinds here; selections store identities, never copied live output.
export type DetailPanelSelection = { sessionId: string; viewKey: string } & (
  | { kind: 'subagent'; agentId: string }
  | { kind: 'changes' }
);

export function resolveDetailPanel(
  selection: DetailPanelSelection | null,
  viewKey: string,
  sessionId: string | undefined,
  conversation: Conversation,
) {
  if (!selection || selection.viewKey !== viewKey || selection.sessionId !== sessionId) return null;
  switch (selection.kind) {
    case 'changes':
      return { kind: selection.kind };
    case 'subagent': {
      const agent = conversation.messages.find(
        (message) => message.sessionId === sessionId && message.agent?.id === selection.agentId,
      )?.agent;
      return agent ? { kind: selection.kind, agent } : null;
    }
  }
}
