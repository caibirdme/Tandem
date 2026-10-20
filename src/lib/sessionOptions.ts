import type { Commands, Message, ModelSelection, PermissionMode, ReasoningEffort } from './types';

/** Only the first user message of a work with known history needs this reminder. */
export function needsWorkPermissionConfirmation({
  projectId,
  sessionId,
  permissionMode,
  historyLoaded,
  messages,
}: {
  projectId: string | null;
  sessionId: string | null;
  permissionMode: PermissionMode;
  historyLoaded: boolean;
  messages: readonly Pick<Message, 'role'>[];
}): boolean {
  return (
    projectId !== null &&
    permissionMode === 'ask' &&
    (sessionId === null || historyLoaded) &&
    !messages.some((message) => message.role === 'user')
  );
}

/** Full authorization belongs to an explicit work session, never to chat. */
export function newSessionPayload(
  projectId: string | null,
  selection: ModelSelection,
  permissionMode: PermissionMode = 'ask',
  reasoningEffort: ReasoningEffort = null,
): Commands['create_session']['payload'] {
  return {
    ...selection,
    projectId,
    permissionMode: projectId === null ? 'ask' : permissionMode,
    reasoningEffort,
  };
}
