import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '../i18n';
import type { ReasoningEffortValue } from '../lib/types';
import { ReasoningEffortSelector } from './ReasoningEffortSelector';

const dropdown = vi.hoisted(() => ({ change: (_value: string) => undefined as void }));

// Render the portaled menu inline so its options and stale event guard can be verified.
vi.mock('@radix-ui/react-dropdown-menu', () => ({
  Root: ({ children }: { children: ReactNode }) => children,
  Portal: ({ children }: { children: ReactNode }) => children,
  Trigger: ({ children, ...props }: { children: ReactNode }) => (
    <button {...props}>{children}</button>
  ),
  Content: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  Label: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  RadioGroup: ({
    children,
    onValueChange,
  }: {
    children: ReactNode;
    onValueChange: (value: string) => void;
  }) => {
    dropdown.change = onValueChange;
    return <div>{children}</div>;
  },
  RadioItem: ({ children, value }: { children: ReactNode; value: string }) => (
    <div data-effort={value}>{children}</div>
  ),
  ItemIndicator: ({ children }: { children: ReactNode }) => <span>{children}</span>,
}));

const props = {
  effort: null,
  defaultEffort: null,
  disabled: false,
  saving: false,
  uncertain: false,
  onChange: vi.fn(),
};

afterEach(() => {
  setLocale('en');
  props.onChange.mockClear();
});

describe('conversation effort menu', () => {
  it.each([
    { supported: null, expected: ['default', 'low', 'medium', 'high', 'xhigh', 'max'] },
    { supported: [], expected: ['default'] },
    { supported: ['low', 'xhigh'], expected: ['default', 'low', 'xhigh'] },
  ] satisfies { supported: ReasoningEffortValue[] | null; expected: string[] }[])(
    'shows the model choices $supported plus its default',
    ({ supported, expected }) => {
      const markup = renderToStaticMarkup(
        <ReasoningEffortSelector {...props} supportedEfforts={supported} />,
      );
      expect([...markup.matchAll(/data-effort="([^"]+)"/g)].map((match) => match[1])).toEqual(
        expected,
      );
    },
  );

  it('rejects stale choices that the newly selected model does not support', () => {
    renderToStaticMarkup(
      <ReasoningEffortSelector {...props} supportedEfforts={['low', 'xhigh']} />,
    );
    dropdown.change('max');
    dropdown.change('HIGH');
    expect(props.onChange).not.toHaveBeenCalled();
    dropdown.change('xhigh');
    dropdown.change('default');
    expect(props.onChange.mock.calls).toEqual([['xhigh'], [null]]);
  });

  it('keeps effort labels raw in a Chinese menu including the configured default', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <ReasoningEffortSelector {...props} effort="xhigh" defaultEffort="high" />,
    );
    expect(markup).toContain('思考强度：xhigh');
    expect(markup).toContain('high');
    for (const effort of ['low', 'medium', 'high', 'xhigh', 'max'])
      expect(markup).toContain(`<span>${effort}</span>`);
    expect(markup).not.toContain('<span>极高</span>');
  });

  it('ignores a choice while the selector is locked', () => {
    renderToStaticMarkup(<ReasoningEffortSelector {...props} disabled />);
    dropdown.change('xhigh');
    expect(props.onChange).not.toHaveBeenCalled();
  });
});
