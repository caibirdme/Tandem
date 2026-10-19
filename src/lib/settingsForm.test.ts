import { describe, expect, it } from 'vitest';

import {
  completeProviderEdit,
  preferencesPayload,
  providerPayload,
  titleModelPayload,
  titleModelValue,
} from './settingsForm';
import type { CredentialEdit, ProviderEditSession } from './settingsForm';
import type { ProviderDraft, Settings } from './types';

const provider: ProviderDraft = {
  id: 'provider-1',
  name: ' My provider ',
  protocol: 'openai-completions',
  baseUrl: ' https://example.com/v1 ',
  models: [
    {
      id: ' model-1 ',
      name: ' ',
      reasoningEffort: null,
      contextWindow: null,
      maxOutputTokens: 100,
    },
  ],
};
const noop = () => undefined;

describe('provider editor save ownership', () => {
  const first: ProviderEditSession = {
    instanceId: 'edit-first',
    provider: { ...provider, hasApiKey: true },
  };

  it('closes the submitting editor after its save completes', () => {
    expect(completeProviderEdit(first, first.instanceId)).toBeNull();
    expect(completeProviderEdit(null, first.instanceId)).toBeNull();
  });

  it.each([
    { name: 'another provider', providerId: 'provider-2' },
    { name: 'the same provider reopened', providerId: provider.id },
  ])('keeps $name open when an older save completes late', async ({ providerId }) => {
    let current: ProviderEditSession | null = first;
    let release: () => void = noop;
    const save = new Promise<void>((resolve) => {
      release = resolve;
    });
    const completion = save.then(() => {
      current = completeProviderEdit(current, first.instanceId);
    });
    const next: ProviderEditSession = {
      instanceId: 'edit-next',
      provider: { ...first.provider, id: providerId },
    };
    current = next;
    release();
    await completion;
    expect(current).toBe(next);
    expect(completeProviderEdit(current, next.instanceId)).toBeNull();
  });
});

describe('provider form serialization', () => {
  it.each([
    { name: 'preserve', edit: { kind: 'keep' }, expected: undefined },
    { name: 'replace', edit: { kind: 'replace', value: 'fixture-key' }, expected: 'fixture-key' },
    {
      name: 'replace with whitespace',
      edit: { kind: 'replace', value: ' \tfixture-key\n' },
      expected: 'fixture-key',
    },
    { name: 'remove', edit: { kind: 'remove' }, expected: '' },
  ] satisfies { name: string; edit: CredentialEdit; expected: string | undefined }[])(
    'encodes the explicit $name credential intent',
    ({ edit, expected }) => {
      const payload = providerPayload(provider, edit);
      expect(payload.apiKey).toBe(expected);
      expect(Object.hasOwn(payload, 'apiKey')).toBe(edit.kind !== 'keep');
    },
  );

  it.each(['', '   ', '\t\n'])(
    'rejects a blank replacement %j without clearing the saved credential',
    (value) => {
      expect(() => providerPayload(provider, { kind: 'replace', value })).toThrow(
        'The replacement key cannot be empty.',
      );
    },
  );

  it('normalizes model identifiers and strips form-only fields without changing the draft', () => {
    const draft = {
      ...provider,
      hasApiKey: true,
      models: provider.models.map((model) => ({ ...model, key: 'ui-row-key' })),
    };
    const payload = providerPayload(draft, { kind: 'keep' });
    expect(payload.provider).toEqual({
      id: provider.id,
      name: 'My provider',
      protocol: provider.protocol,
      baseUrl: 'https://example.com/v1',
      models: [
        {
          id: 'model-1',
          name: 'model-1',
          reasoningEffort: null,
          contextWindow: null,
          maxOutputTokens: 100,
        },
      ],
    });
    expect(draft.name).toBe(' My provider ');
    expect(draft.models[0]?.key).toBe('ui-row-key');
  });

  it.each([
    { name: 'an empty model list', ids: [], message: 'Add at least one model.' },
    { name: 'a blank model ID', ids: [' '], message: 'Enter an ID for every model.' },
    {
      name: 'duplicate IDs after trimming',
      ids: ['same', ' same '],
      message: 'Model IDs must be unique within a provider.',
    },
  ])('rejects $name', ({ ids, message }) => {
    const models = ids.map((id) => ({
      id,
      name: '',
      reasoningEffort: null,
      contextWindow: null,
      maxOutputTokens: null,
    }));
    expect(() => providerPayload({ ...provider, models }, { kind: 'keep' })).toThrow(message);
  });
});

describe('preferences form ownership', () => {
  const settings: Settings = {
    providers: [],
    titleModel: { providerId: 'title-provider', modelId: 'title-model' },
    cliPath: '/latest/copilot',
    telemetry: {
      enabled: true,
      endpoint: 'http://latest-collector:4318',
      protocol: 'http/json',
      captureContent: false,
    },
  };

  it('saves only the chosen title model and never submits telemetry or CLI preferences', () => {
    const configured = { ...provider, hasApiKey: true };
    const selection = { providerId: configured.id, modelId: configured.models[0]?.id ?? '' };
    expect(titleModelPayload(titleModelValue(selection), [configured])).toEqual({
      titleModel: selection,
    });
    expect(titleModelPayload('', [configured])).toEqual({ titleModel: null });
    expect(() =>
      titleModelPayload(titleModelValue({ ...selection, modelId: 'removed' }), [configured]),
    ).toThrow('The selected title model is unavailable');
  });

  it('preserves the latest CLI path while editing telemetry', () => {
    const telemetry = { ...settings.telemetry, endpoint: 'http://new-collector:4318' };
    expect(preferencesPayload(settings, { kind: 'telemetry', value: telemetry })).toEqual({
      telemetry,
      cliPath: settings.cliPath,
    });
  });

  it.each([
    { input: ' /new/copilot ', expected: '/new/copilot' },
    { input: '  ', expected: null },
  ])(
    'preserves the latest telemetry when the CLI path becomes $expected',
    ({ input, expected }) => {
      expect(preferencesPayload(settings, { kind: 'cliPath', value: input })).toEqual({
        telemetry: settings.telemetry,
        cliPath: expected,
      });
    },
  );
});
