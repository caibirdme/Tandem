import { describe, expect, it } from 'vitest';

import { changeModelFormId, initialModelForm, submitModelForm } from './modelForm';
import type { ModelEditorDraft, ModelFormError } from './modelForm';
import type { ModelFormDraft } from './settingsForm';

const saved: ModelFormDraft = {
  id: 'opus',
  name: 'Opus',
  contextWindowK: '1000',
  maxOutputTokens: 128_000,
  reasoningEffort: null,
  supportedReasoningEfforts: ['low', 'xhigh'],
};

describe('model modal drafts', () => {
  it('starts manual entry with unset parameters and does not invent model limits', () => {
    expect(initialModelForm(null)).toEqual({
      id: '',
      name: '',
      contextWindowK: '',
      maxOutputTokens: '',
      reasoningEffort: null,
    });
  });

  it('isolates unsaved changes and capability arrays from the provider model', () => {
    const draft = initialModelForm(saved);
    draft.name = 'Changed';
    draft.supportedReasoningEfforts?.push('max');
    expect(saved.name).toBe('Opus');
    expect(saved.supportedReasoningEfforts).toEqual(['low', 'xhigh']);
    expect(initialModelForm(saved).maxOutputTokens).toBe('128000');
  });

  it('drops model-specific capabilities and the override when the model ID changes', () => {
    const draft = { ...initialModelForm(saved), reasoningEffort: 'xhigh' as const };
    expect(changeModelFormId(draft, draft.id)).toBe(draft);
    expect(changeModelFormId(draft, 'another')).toMatchObject({
      id: 'another',
      supportedReasoningEfforts: null,
      reasoningEffort: null,
    });
  });

  it('normalizes a valid save without mutating the form or losing token precision', () => {
    const draft = {
      ...initialModelForm(saved),
      id: ' model ',
      name: ' ',
      contextWindowK: '131.0720',
      maxOutputTokens: '32e3',
    };
    expect(submitModelForm(draft, [])).toEqual({
      model: {
        ...saved,
        id: 'model',
        name: 'model',
        contextWindowK: '131.072',
        maxOutputTokens: 32_000,
      },
    });
    expect(draft.id).toBe(' model ');
    const result = submitModelForm(draft, []);
    if ('model' in result) result.model.supportedReasoningEfforts?.push('max');
    expect(draft.supportedReasoningEfforts).toEqual(['low', 'xhigh']);
  });

  it.each([
    { patch: { id: ' ' }, expected: 'error.blankModelId' },
    { patch: { contextWindowK: '' }, expected: 'error.modelContextRange' },
    { patch: { contextWindowK: '0' }, expected: 'error.modelContextRange' },
    { patch: { contextWindowK: '0.0001' }, expected: 'error.modelContextRange' },
    { patch: { contextWindowK: '10000.001' }, expected: 'error.modelContextRange' },
    { patch: { maxOutputTokens: '0' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '-1' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '1.5' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '10000001' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '1e999' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: 'NaN' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '0x10' }, expected: 'error.modelOutputRange' },
    { patch: { maxOutputTokens: '1000000' }, expected: 'error.modelOutputContext' },
    { patch: { maxOutputTokens: '1000001' }, expected: 'error.modelOutputContext' },
    { patch: { reasoningEffort: 'high' }, expected: 'error.modelReasoningUnsupported' },
    {
      patch: { reasoningEffort: 'low', supportedReasoningEfforts: [] },
      expected: 'error.modelReasoningUnsupported',
    },
  ] satisfies { patch: Partial<ModelEditorDraft>; expected: ModelFormError }[])(
    'keeps invalid parameters editable: $patch',
    ({ patch, expected }) => {
      expect(submitModelForm({ ...initialModelForm(saved), ...patch }, [])).toEqual({
        error: expected,
      });
    },
  );

  it('rejects a duplicate trimmed ID while the editing model can be excluded by the parent', () => {
    expect(submitModelForm(initialModelForm(saved), [' opus '])).toEqual({
      error: 'error.duplicateModelIds',
    });
    expect(submitModelForm(initialModelForm(saved), ['another'])).toEqual({ model: saved });
  });

  it.each([
    { contextWindowK: '0.001', maxOutputTokens: '', expected: null },
    { contextWindowK: '10000', maxOutputTokens: '9999999', expected: 9_999_999 },
  ])('accepts valid boundary limits $contextWindowK K', (limits) => {
    const result = submitModelForm({ ...initialModelForm(saved), ...limits }, []);
    expect(result).toMatchObject({ model: { maxOutputTokens: limits.expected } });
  });
});
