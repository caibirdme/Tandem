import type { Commands, ModelSelection, PermissionMode } from './types';

/** Full authorization belongs to an explicit work session, never to chat. */
export function newSessionPayload(
  projectId: string | null,
  selection: ModelSelection,
  permissionMode: PermissionMode = 'ask',
): Commands['create_session']['payload'] {
  return {
    ...selection,
    projectId,
    permissionMode: projectId === null ? 'ask' : permissionMode,
  };
}
