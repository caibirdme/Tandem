import { errorMessage } from './api';
import { createSessionSettingChanges } from './sessionSettingChanges';
import type { ModelSelection } from './types';

export type ModelChangeEvent =
  | { type: 'modelChangeStarted'; sessionId: string }
  | { type: 'modelChanged'; sessionId: string; selection: ModelSelection; notice?: string }
  | { type: 'modelUnconfirmed'; sessionId: string; error: string };

interface Dependencies {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  update: (event: ModelChangeEvent) => void;
  save: (sessionId: string, selection: ModelSelection) => Promise<ModelSelection>;
  read: (sessionId: string) => Promise<ModelSelection>;
}

export function createModelChanges(dependencies: Dependencies) {
  const { update } = dependencies;
  return createSessionSettingChanges({
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
    uncertainMessage: '当前模型尚未确认，请先重新确认模型。',
    timeoutMessage: '模型确认超时，请重试。',
    saveFailure: (reason) => `模型切换未获确认：${errorMessage(reason)}。`,
    readFailure: (reason, failure) =>
      `${failure ? `${failure} ` : ''}无法确认当前模型：${errorMessage(reason)}。确认前暂不能发送或切换模型。`,
    recovered: (selection, failure) => `${failure} 已重新确认当前模型为「${selection.modelId}」。`,
  });
}
