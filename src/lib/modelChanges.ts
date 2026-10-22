import { translate as t } from '../i18n';
import { errorMessage } from './api';
import { createSessionSettingChanges } from './sessionSettingChanges';
import type { ModelSelection, ReasoningEffort } from './types';

export interface ConfirmedModelSelection extends ModelSelection {
  reasoningEffort: ReasoningEffort;
}

export type ModelChangeEvent =
  | { type: 'modelChangeStarted'; sessionId: string }
  | { type: 'modelChanged'; sessionId: string; selection: ConfirmedModelSelection; notice?: string }
  | { type: 'modelUnconfirmed'; sessionId: string; error: string };

interface Dependencies {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  update: (event: ModelChangeEvent) => void;
  save: (sessionId: string, selection: ModelSelection) => Promise<ConfirmedModelSelection>;
  read: (sessionId: string) => Promise<ConfirmedModelSelection>;
}

export function createModelChanges(dependencies: Dependencies) {
  const { update } = dependencies;
  return createSessionSettingChanges<ConfirmedModelSelection, ModelSelection>({
    ...dependencies,
    started: (sessionId) => update({ type: 'modelChangeStarted', sessionId }),
    confirmed: (sessionId, selection, notice) =>
      update({
        type: 'modelChanged',
        sessionId,
        selection,
        ...(notice ? { notice } : {}),
      }),
    unconfirmed: (sessionId, error) => update({ type: 'modelUnconfirmed', sessionId, error }),
    get uncertainMessage() {
      return t('error.modelUnconfirmed');
    },
    get timeoutMessage() {
      return t('error.modelTimeout');
    },
    saveFailure: (reason) => t('error.modelSaveFailed', { error: errorMessage(reason) }),
    readFailure: (reason, failure) =>
      t('error.modelConfirmationFailed', {
        failure: failure ? `${failure} ` : '',
        error: errorMessage(reason),
      }),
    recovered: (selection, failure) =>
      t('error.modelRecovered', { failure, model: selection.modelId }),
  });
}
