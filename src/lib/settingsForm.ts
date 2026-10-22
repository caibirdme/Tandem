import { translate as t } from '../i18n';
import type {
  Commands,
  DiscoveredModel,
  Model,
  ModelSelection,
  Provider,
  ProviderDraft,
  ProviderProtocol,
  Settings,
  Telemetry,
} from './types';

export interface ProviderEditSession {
  instanceId: string;
  provider: Provider | null;
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
  throw new Error(t('error.titleModelUnavailable'));
}

export type ModelFormDraft = Omit<Model, 'contextWindow'> & { contextWindowK: string };
export type ProviderFormDraft = Omit<ProviderDraft, 'models'> & { models: ModelFormDraft[] };

export function discoveredModelDraft(model: DiscoveredModel): ModelFormDraft {
  return {
    id: model.id,
    name: model.displayName,
    reasoningEffort: null,
    contextWindowK:
      model.contextWindow === undefined ? '' : contextWindowKilotokens(model.contextWindow),
    maxOutputTokens: model.maxOutputTokens ?? null,
    ...(model.supportedReasoningEfforts === undefined
      ? {}
      : { supportedReasoningEfforts: [...model.supportedReasoningEfforts] }),
  };
}

export function contextWindowKilotokens(tokens: number): string {
  const count = BigInt(tokens);
  const fraction = String(count % 1_000n)
    .padStart(3, '0')
    .replace(/0+$/, '');
  return `${count / 1_000n}${fraction ? `.${fraction}` : ''}`;
}

/** K is decimal thousands; retain single-token precision when editing saved limits. */
export function contextWindowTokens(value: string): number {
  const parts = /^(\d*)(?:\.(\d*))?(?:e([+-]?\d+))?$/i.exec(value.trim());
  const digits = `${parts?.[1] ?? ''}${parts?.[2] ?? ''}`;
  const shift = 3 - (parts?.[2]?.length ?? 0) + Number(parts?.[3] ?? 0);
  if (!digits || !Number.isSafeInteger(shift) || shift > 16 || shift < -digits.length) {
    throw new Error(t('error.invalidContextWindow'));
  }
  const coefficient = BigInt(digits);
  const factor = 10n ** BigInt(Math.abs(shift));
  const tokens = shift < 0 ? coefficient / factor : coefficient * factor;
  if (
    (shift < 0 && coefficient % factor !== 0n) ||
    tokens <= 0n ||
    tokens > BigInt(Number.MAX_SAFE_INTEGER)
  ) {
    throw new Error(t('error.invalidContextWindow'));
  }
  return Number(tokens);
}

const protocolPaths: Record<ProviderProtocol, string> = {
  anthropic: '/v1/messages',
  'openai-responses': '/responses',
  'openai-completions': '/chat/completions',
};

export function providerRequestUrl(protocol: ProviderProtocol, baseUrl: string): string {
  const base = baseUrl.trim().replace(/\/+$/, '');
  return base ? `${base}${protocolPaths[protocol]}` : '';
}

export type CredentialEdit =
  | { kind: 'keep' }
  | { kind: 'replace'; value: string }
  | { kind: 'remove' };

/** Explicit fields keep UI-only row keys and saved-key metadata out of the command. */
export function providerPayload(
  draft: ProviderFormDraft,
  credential: CredentialEdit,
): Commands['save_provider']['payload'] {
  const apiKey = credential.kind === 'replace' ? credential.value.trim() : '';
  if (credential.kind === 'replace' && !apiKey) {
    throw new Error(t('error.blankReplacement'));
  }
  if (draft.models.length === 0) throw new Error(t('error.noModels'));
  if (draft.models.some((model) => !model.id.trim())) throw new Error(t('error.blankModelId'));
  if (new Set(draft.models.map((model) => model.id.trim())).size !== draft.models.length) {
    throw new Error(t('error.duplicateModelIds'));
  }
  const models = draft.models.map((model) => ({
    id: model.id.trim(),
    name: model.name.trim() || model.id.trim(),
    reasoningEffort: model.reasoningEffort,
    contextWindow: contextWindowTokens(model.contextWindowK),
    maxOutputTokens: model.maxOutputTokens,
    ...(model.supportedReasoningEfforts === undefined
      ? {}
      : { supportedReasoningEfforts: model.supportedReasoningEfforts }),
  }));
  return {
    provider: {
      id: draft.id,
      name: draft.name.trim(),
      protocol: draft.protocol,
      baseUrl: draft.baseUrl.trim(),
      listModelsPath: draft.listModelsPath.trim() || '/v1/models',
      models,
    },
    ...(credential.kind === 'keep' ? {} : { apiKey }),
  };
}

type PreferenceEdit =
  | { kind: 'telemetry'; value: Telemetry; credential?: CredentialEdit }
  | { kind: 'cliPath'; value: string };

/** Only the visible settings page owns a draft; other values come from the latest snapshot. */
export function preferencesPayload(
  settings: Settings,
  edit: PreferenceEdit,
): Commands['save_preferences']['payload'] {
  if (edit.kind === 'telemetry' && edit.value.agentlens) {
    if (edit.value.enabled && !edit.value.agentlens.serviceName.trim()) {
      throw new Error(t('error.telemetryServiceName'));
    }
    if (edit.credential?.kind === 'replace' && !edit.credential.value.trim()) {
      throw new Error(t('error.blankReplacement'));
    }
  }
  return {
    telemetry:
      edit.kind === 'telemetry'
        ? {
            ...edit.value,
            endpoint: edit.value.endpoint.trim(),
            ...(edit.value.agentlens
              ? {
                  agentlens: { serviceName: edit.value.agentlens.serviceName.trim() },
                  protocol: 'http/json' as const,
                }
              : {}),
          }
        : settings.telemetry,
    cliPath: edit.kind === 'cliPath' ? edit.value.trim() || null : settings.cliPath,
    ...(edit.kind === 'telemetry' && edit.value.agentlens && edit.credential?.kind === 'replace'
      ? { telemetryApiKey: edit.credential.value.trim() }
      : edit.kind === 'telemetry' && edit.value.agentlens && edit.credential?.kind === 'remove'
        ? { telemetryApiKey: '' }
        : {}),
  };
}
