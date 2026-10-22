import { describe, expect, it } from 'vitest';

import {
  completeProviderEdit,
  contextWindowKilotokens,
  contextWindowTokens,
  preferencesPayload,
  providerPayload,
  providerRequestUrl,
  titleModelPayload,
  titleModelValue,
} from './settingsForm';
import type { CredentialEdit, ProviderEditSession, ProviderFormDraft } from './settingsForm';
import type { Provider, ProviderProtocol, Settings } from './types';

const provider: ProviderFormDraft = {
  id: 'provider-1',
  name: ' My provider ',
  protocol: 'openai-completions',
  baseUrl: ' https://example.com/v1 ',
  listModelsPath: '/v1/models',
  models: [
    {
      id: ' model-1 ',
      name: ' ',
      reasoningEffort: null,
      contextWindowK: '256',
      maxOutputTokens: 100,
    },
  ],
};
const savedProvider: Provider = {
  ...providerPayload(provider, { kind: 'keep' }).provider,
  hasApiKey: true,
};
const noop = () => undefined;

describe('provider editor save ownership', () => {
  const first: ProviderEditSession = {
    instanceId: 'edit-first',
    provider: savedProvider,
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
      provider: { ...savedProvider, id: providerId },
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
    { path: '', expected: '/v1/models' },
    { path: ' /custom/models?region=eu ', expected: '/custom/models?region=eu' },
  ])('persists the normalized discovery path $path', ({ path, expected }) => {
    expect(
      providerPayload({ ...provider, listModelsPath: path }, { kind: 'keep' }).provider
        .listModelsPath,
    ).toBe(expected);
  });

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
      listModelsPath: '/v1/models',
      models: [
        {
          id: 'model-1',
          name: 'model-1',
          reasoningEffort: null,
          contextWindow: 256_000,
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
      contextWindowK: '256',
      maxOutputTokens: null,
    }));
    expect(() => providerPayload({ ...provider, models }, { kind: 'keep' })).toThrow(message);
  });
});

describe('context window input', () => {
  it.each([
    { input: '256', tokens: 256_000 },
    { input: '500', tokens: 500_000 },
    { input: '1024', tokens: 1_024_000 },
    { input: '1.001', tokens: 1_001 },
    { input: '131.072', tokens: 131_072 },
    { input: '0.001', tokens: 1 },
    { input: '9007199254740.991', tokens: Number.MAX_SAFE_INTEGER },
  ])('converts $input K to $tokens tokens and preserves it when reopened', ({ input, tokens }) => {
    expect(contextWindowTokens(input)).toBe(tokens);
    expect(contextWindowKilotokens(tokens)).toBe(input);
    expect(
      providerPayload(
        {
          ...provider,
          models: provider.models.map((model) => ({ ...model, contextWindowK: input })),
        },
        { kind: 'keep' },
      ).provider.models[0]?.contextWindow,
    ).toBe(tokens);
  });

  it('accepts numeric exponent notation without losing token precision', () => {
    expect(contextWindowTokens('1e3')).toBe(1_000_000);
    expect(contextWindowTokens('1001e-3')).toBe(1_001);
  });

  it.each([
    '',
    ' ',
    '0',
    '-1',
    'abc',
    'Infinity',
    '0.0001',
    '256.0001',
    '9007199254741',
    '1e999',
    '1e-999',
  ])('rejects a missing, nonpositive, fractional-token, or unsafe limit %j', (value) => {
    expect(() => contextWindowTokens(value)).toThrow('Enter a positive context window');
    expect(() =>
      providerPayload(
        {
          ...provider,
          models: provider.models.map((model) => ({ ...model, contextWindowK: value })),
        },
        { kind: 'keep' },
      ),
    ).toThrow('Enter a positive context window');
  });
});

describe('provider request URL preview', () => {
  it.each([
    {
      protocol: 'anthropic',
      base: 'https://example.com/x',
      expected: 'https://example.com/x/v1/messages',
    },
    {
      protocol: 'anthropic',
      base: ' https://example.com/x/// ',
      expected: 'https://example.com/x/v1/messages',
    },
    {
      protocol: 'anthropic',
      base: 'https://example.com/x/v1',
      expected: 'https://example.com/x/v1/v1/messages',
    },
    {
      protocol: 'openai-responses',
      base: 'https://example.com/x/',
      expected: 'https://example.com/x/responses',
    },
    {
      protocol: 'openai-completions',
      base: 'https://example.com/v1',
      expected: 'https://example.com/v1/chat/completions',
    },
  ] satisfies { protocol: ProviderProtocol; base: string; expected: string }[])(
    'previews $protocol requests from $base',
    ({ protocol, base, expected }) => {
      expect(providerRequestUrl(protocol, base)).toBe(expected);
    },
  );

  it('does not show a relative endpoint before a base URL is entered', () => {
    expect(providerRequestUrl('anthropic', ' ')).toBe('');
  });
});

describe('preferences form ownership', () => {
  const settings: Settings = {
    providers: [],
    titleModel: { providerId: 'title-provider', modelId: 'title-model' },
    cliPath: '/latest/copilot',
    fontSize: 'medium',
    telemetry: {
      enabled: true,
      endpoint: 'http://latest-collector:4318',
      protocol: 'http/json',
      captureContent: false,
    },
  };

  it('saves only the chosen title model and never submits telemetry or CLI preferences', () => {
    const configured = savedProvider;
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
