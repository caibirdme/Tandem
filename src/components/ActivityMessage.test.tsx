import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { ActivityMessage } from './ActivityMessage';

function heading(names: string[], content = '') {
  const markup = renderToStaticMarkup(
    <ActivityMessage
      content={content}
      activity={{
        status: 'running',
        tools: names.map((name, index) => ({
          toolCallId: String(index),
          name,
          status: 'running',
        })),
      }}
    />,
  );
  return markup.match(/<span class="activity-title">(.*?)<\/span>/)?.[1];
}

afterEach(() => setLocale('en'));

describe('conservative activity fallback headings', () => {
  it.each([
    { names: ['view', 'builtin:view'], expected: 'Reading files' },
    { names: ['grep', 'glob'], expected: 'Searching the project' },
    { names: ['view', 'grep', 'glob'], expected: 'Inspecting the project' },
    { names: ['create', 'edit', 'apply_patch'], expected: 'Updating files' },
    {
      names: [
        'bash',
        'powershell',
        'read_bash',
        'write_bash',
        'stop_bash',
        'read_powershell',
        'write_powershell',
        'stop_powershell',
      ],
      expected: 'Running commands',
    },
    { names: ['web_fetch'], expected: 'Reading web pages' },
    { names: ['mcp_custom_tool'], expected: 'Agent activity' },
    { names: ['str_replace_editor'], expected: 'Agent activity' },
    { names: ['view', 'mcp_custom_tool'], expected: 'Agent activity' },
    { names: ['view', 'bash'], expected: 'Agent activity' },
    { names: ['web_fetch', 'edit'], expected: 'Agent activity' },
    { names: [], expected: 'Agent activity' },
  ])('uses $expected for $names', ({ names, expected }) => {
    expect(heading(names, '  ')).toBe(expected);
  });

  it('always prefers the reported intent over tool categories', () => {
    expect(heading(['view', 'grep'], 'Tracing how approvals are requested')).toBe(
      'Tracing how approvals are requested',
    );
  });

  it('translates inferred headings using the selected locale', () => {
    setLocale('zh-CN');
    expect(heading(['view'])).toBe('读取文件');
    expect(heading(['view', 'glob'])).toBe('检查项目');
    expect(heading(['unknown'])).toBe('Agent 活动');
  });
});
