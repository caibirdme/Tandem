import type {
  Commands,
  ModelSelection,
  Provider,
  ProviderDraft,
  Settings,
  Telemetry,
} from './types';

export interface ProviderEditSession {
  instanceId: string;
  provider: Provider;
}

/** A completed save owns only the editor instance that submitted it. */
export function completeProviderEdit(
  current: ProviderEditSession | null,
  instanceId: string,
): ProviderEditSession | null {
  return current?.instanceId === instanceId ? null : current;
}

export function titleModelValue(selection: ModelSelection | null): string {
  return selection ? JSON.stringify([selection.providerId, selection.modelId]) : '';
}

/** Title preferences have their own command and never carry unrelated settings. */
export function titleModelPayload(
  value: string,
  providers: Provider[],
): Commands['save_title_model']['payload'] {
  if (!value) return { titleModel: null };
  for (const provider of providers) {
    for (const model of provider.models) {
      const selection = { providerId: provider.id, modelId: model.id };
      if (titleModelValue(selection) === value) return { titleModel: selection };
    }
  }
  throw new Error('所选标题模型已不可用，请重新选择。');
}

export type CredentialEdit =
  | { kind: 'keep' }
  | { kind: 'replace'; value: string }
  | { kind: 'remove' };

/** Explicit fields keep UI-only row keys and saved-key metadata out of the command. */
export function providerPayload(
  draft: ProviderDraft,
  credential: CredentialEdit,
): Commands['save_provider']['payload'] {
  const apiKey = credential.kind === 'replace' ? credential.value.trim() : '';
  if (credential.kind === 'replace' && !apiKey) {
    throw new Error('替换密钥不能为空。');
  }
  const models = draft.models.map((model) => ({
    id: model.id.trim(),
    name: model.name.trim() || model.id.trim(),
    reasoningEffort: model.reasoningEffort,
    contextWindow: model.contextWindow,
    maxOutputTokens: model.maxOutputTokens,
  }));
  if (models.length === 0) throw new Error('请至少添加一个模型。');
  if (models.some((model) => !model.id)) throw new Error('请填写每个模型的 ID。');
  if (new Set(models.map((model) => model.id)).size !== models.length) {
    throw new Error('同一供应商下的模型 ID 不能重复。');
  }
  return {
    provider: {
      id: draft.id,
      name: draft.name.trim(),
      protocol: draft.protocol,
      baseUrl: draft.baseUrl.trim(),
      models,
    },
    ...(credential.kind === 'keep' ? {} : { apiKey }),
  };
}

type PreferenceEdit = { kind: 'telemetry'; value: Telemetry } | { kind: 'cliPath'; value: string };

/** Only the visible settings page owns a draft; other values come from the latest snapshot. */
export function preferencesPayload(
  settings: Settings,
  edit: PreferenceEdit,
): Commands['save_preferences']['payload'] {
  return {
    telemetry: edit.kind === 'telemetry' ? edit.value : settings.telemetry,
    cliPath: edit.kind === 'cliPath' ? edit.value.trim() || null : settings.cliPath,
  };
}
