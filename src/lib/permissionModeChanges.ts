import { translate as t } from '../i18n';
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
    get uncertainMessage() {
      return t('error.permissionsUnconfirmed');
    },
    get timeoutMessage() {
      return t('error.permissionsTimeout');
    },
    saveFailure: (reason) => t('error.permissionSaveFailed', { error: errorMessage(reason) }),
    readFailure: (reason, failure) =>
      t('error.permissionConfirmationFailed', {
        failure: failure ? `${failure} ` : '',
        error: errorMessage(reason),
      }),
    recovered: (permissionMode, failure) => {
      const label = permissionMode === 'allow-all' ? t('permission.allowAll') : t('permission.ask');
      return t('error.permissionRecovered', { failure, mode: label });
    },
  });
}
