import type { Commands, ModelSelection, PermissionMode, ReasoningEffort } from './types';

/** Work defaults to full access; Chat retains its web-only runtime boundary. */
export function newSessionPayload(
  projectId: string | null,
  selection: ModelSelection,
  permissionMode: PermissionMode = 'allow-all',
  reasoningEffort: ReasoningEffort = null,
): Commands['create_session']['payload'] {
  return {
    ...selection,
    projectId,
    permissionMode: projectId === null ? 'ask' : permissionMode,
    reasoningEffort,
  };
}
