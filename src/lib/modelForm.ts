import { reasoningEffortOptions } from './reasoningEffort';
import { contextWindowKilotokens, contextWindowTokens, type ModelFormDraft } from './settingsForm';

export type ModelEditorDraft = Omit<ModelFormDraft, 'maxOutputTokens'> & {
  maxOutputTokens: string;
};

export type ModelFormError =
  | 'error.blankModelId'
  | 'error.duplicateModelIds'
  | 'error.modelContextRange'
  | 'error.modelOutputRange'
  | 'error.modelOutputContext'
  | 'error.modelReasoningUnsupported';

/** Keep transient input and capability arrays separate from the provider draft. */
export function initialModelForm(model: ModelFormDraft | null): ModelEditorDraft {
  return model
    ? {
        ...model,
        maxOutputTokens: model.maxOutputTokens?.toString() ?? '',
        ...(model.supportedReasoningEfforts
          ? { supportedReasoningEfforts: [...model.supportedReasoningEfforts] }
          : {}),
      }
    : {
        id: '',
        name: '',
        contextWindowK: '',
        maxOutputTokens: '',
        reasoningEffort: null,
      };
}

export function changeModelFormId(draft: ModelEditorDraft, id: string): ModelEditorDraft {
  return id === draft.id
    ? draft
    : { ...draft, id, supportedReasoningEfforts: null, reasoningEffort: null };
}

/** Validate before the modal closes so incomplete parameters stay editable. */
export function submitModelForm(
  draft: ModelEditorDraft,
  existingIds: readonly string[],
): { model: ModelFormDraft } | { error: ModelFormError } {
  const id = draft.id.trim();
  if (!id) return { error: 'error.blankModelId' };
  if (existingIds.some((existing) => existing.trim() === id))
    return { error: 'error.duplicateModelIds' };
  let context: number;
  try {
    context = contextWindowTokens(draft.contextWindowK);
  } catch {
    return { error: 'error.modelContextRange' };
  }
  if (context > 10_000_000) return { error: 'error.modelContextRange' };
  const outputText = draft.maxOutputTokens.trim();
  const output = outputText ? Number(outputText) : null;
  if (
    output !== null &&
    (!/^(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(outputText) ||
      !Number.isSafeInteger(output) ||
      output < 1 ||
      output > 10_000_000)
  )
    return { error: 'error.modelOutputRange' };
  if (output !== null && output >= context) return { error: 'error.modelOutputContext' };
  if (
    draft.reasoningEffort !== null &&
    !reasoningEffortOptions(draft.supportedReasoningEfforts).includes(draft.reasoningEffort)
  )
    return { error: 'error.modelReasoningUnsupported' };
  return {
    model: {
      ...draft,
      id,
      name: draft.name.trim() || id,
      contextWindowK: contextWindowKilotokens(context),
      maxOutputTokens: output,
      ...(draft.supportedReasoningEfforts
        ? { supportedReasoningEfforts: [...draft.supportedReasoningEfforts] }
        : {}),
    },
  };
}
