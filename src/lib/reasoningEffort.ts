import type { ReasoningEffort, ReasoningEffortValue } from './types';

export const reasoningEffortPresets = ['low', 'medium', 'high', 'xhigh', 'max'] as const;

/** Missing capabilities are unknown; an empty list explicitly disables overrides. */
export function reasoningEffortOptions(
  supported: readonly ReasoningEffortValue[] | null | undefined,
): readonly ReasoningEffortValue[] {
  return supported ?? reasoningEffortPresets;
}

export function isReasoningEffort(value: unknown): value is ReasoningEffort {
  return value === null || reasoningEffortPresets.some((effort) => effort === value);
}

/** A draft follows the model default when its previous override is unavailable. */
export function supportedReasoningEffort(
  effort: ReasoningEffort,
  supported: readonly ReasoningEffortValue[] | null | undefined,
): ReasoningEffort {
  return effort !== null && !reasoningEffortOptions(supported).includes(effort) ? null : effort;
}
