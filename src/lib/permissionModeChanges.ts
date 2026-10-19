import { errorMessage } from './api';
import { createSessionSettingChanges } from './sessionSettingChanges';
import type { PermissionMode } from './types';

export type PermissionModeChangeEvent =
  | { type: 'permissionModeChangeStarted'; sessionId: string }
  | {
      type: 'permissionModeChanged';
      sessionId: string;
      permissionMode: PermissionMode;
      notice?: string;
    }
  | { type: 'permissionModeUnconfirmed'; sessionId: string; error: string };

interface Dependencies {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  update: (event: PermissionModeChangeEvent) => void;
  save: (sessionId: string, mode: PermissionMode) => Promise<PermissionMode>;
  read: (sessionId: string) => Promise<PermissionMode>;
}

/** A lost save response cannot release the send lock until the server's mode is known. */
export function createPermissionModeChanges(dependencies: Dependencies) {
  const { update } = dependencies;
  return createSessionSettingChanges({
    ...dependencies,
    started: (sessionId) => update({ type: 'permissionModeChangeStarted', sessionId }),
    confirmed: (sessionId, permissionMode, notice) =>
      update({
        type: 'permissionModeChanged',
        sessionId,
        permissionMode,
        ...(notice ? { notice } : {}),
      }),
    unconfirmed: (sessionId, error) =>
      update({ type: 'permissionModeUnconfirmed', sessionId, error }),
    uncertainMessage: '工具权限尚未确认，请先重新确认权限。',
    timeoutMessage: '权限确认超时，请重试。',
    saveFailure: (reason) => `权限更新未获确认：${errorMessage(reason)}。`,
    readFailure: (reason, failure) =>
      `${failure ? `${failure} ` : ''}无法确认当前工具权限：${errorMessage(reason)}。确认前暂不能发送或切换权限。`,
    recovered: (permissionMode, failure) => {
      const label = permissionMode === 'allow-all' ? '完全授权' : '逐次询问';
      return `${failure} 已重新确认当前权限为「${label}」。`;
    },
  });
}
