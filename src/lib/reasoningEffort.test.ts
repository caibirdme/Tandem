import { describe, expect, it } from 'vitest';

import {
  isReasoningEffort,
  reasoningEffortOptions,
  supportedReasoningEffort,
} from './reasoningEffort';
import type { ReasoningEffortValue } from './types';

describe('model reasoning capabilities', () => {
  it.each([undefined, null])('uses all presets when capabilities are unknown (%s)', (supported) => {
    expect(reasoningEffortOptions(supported)).toEqual(['low', 'medium', 'high', 'xhigh', 'max']);
  });

  it.each([
    { supported: [], expected: [] },
    { supported: ['low', 'xhigh'], expected: ['low', 'xhigh'] },
  ] satisfies { supported: ReasoningEffortValue[]; expected: ReasoningEffortValue[] }[])(
    'uses only the advertised choices $supported',
    ({ supported, expected }) => {
      expect(reasoningEffortOptions(supported)).toEqual(expected);
    },
  );

  it('clears an unavailable draft override when changing models and preserves supported values', () => {
    expect(supportedReasoningEffort('max', ['low', 'xhigh'])).toBeNull();
    expect(supportedReasoningEffort('xhigh', ['low', 'xhigh'])).toBe('xhigh');
    expect(supportedReasoningEffort('low', [])).toBeNull();
    expect(supportedReasoningEffort(null, [])).toBeNull();
    expect(supportedReasoningEffort('xhigh', undefined)).toBe('xhigh');
  });

  it('accepts the protocol values including xhigh without accepting translated or padded values', () => {
    for (const effort of [null, 'low', 'medium', 'high', 'xhigh', 'max'])
      expect(isReasoningEffort(effort)).toBe(true);
    for (const effort of [undefined, '', 'HIGH', ' high', '极高', 'unsupported', 1])
      expect(isReasoningEffort(effort)).toBe(false);
  });
});
