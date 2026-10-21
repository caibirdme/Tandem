import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { ToolActivity } from '../lib/types';
import { ActivityMessage } from './ActivityMessage';

function activityMarkup(names: string[], content = '') {
  return renderToStaticMarkup(
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
}

function heading(names: string[], content = '') {
  return activityMarkup(names, content).match(/<span class="activity-title">(.*?)<\/span>/)?.[1];
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

  it('keeps the reported intent above the tool category disclosure', () => {
    const content = 'Tracing how approvals are requested';
    const markup = activityMarkup(['view', 'grep'], content);
    expect(markup).toContain(
      `<p class="activity-intent">${content}</p><details class="activity-group">`,
    );
    expect(heading(['view', 'grep'], content)).toBe('Inspecting the project');
  });

  it('translates inferred headings using the selected locale', () => {
    setLocale('zh-CN');
    expect(heading(['view'])).toBe('读取文件');
    expect(heading(['view', 'glob'])).toBe('检查项目');
    expect(heading(['unknown'])).toBe('Agent 活动');
  });
});

describe('activity category disclosure', () => {
  it.each([
    { status: 'running', label: 'Running' },
    { status: 'completed', label: 'Completed' },
    { status: 'error', label: 'Failed' },
    { status: 'interrupted', label: 'Stopped' },
  ] as const)(
    'exposes the $status state without adding visible status or counts',
    ({ status, label }) => {
      const markup = renderToStaticMarkup(
        <ActivityMessage
          content="Checking the conversation layout"
          activity={{
            status,
            tools: [{ toolCallId: 'check-1', name: 'check_layout', status }],
          }}
        />,
      );
      const summary = markup.match(
        /<details class="activity-group"><summary>(.*?)<\/summary>/,
      )?.[1];

      expect(markup).toContain(`class="activity-message ${status}"`);
      expect(markup).toContain(
        '<p class="activity-intent">Checking the conversation layout</p><details class="activity-group">',
      );
      expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
      expect(summary).toMatch(/^<svg\b[^>]*aria-hidden="true"[^>]*>/);
      expect(summary).toMatch(/class="[^"]*activity-chevron[^"]*"/);
      expect(summary).toContain('</svg><span class="activity-title">Agent activity</span>');
      expect(summary).not.toContain('Checking the conversation layout');
      expect(summary).toContain(
        `<span class="activity-group-status"><span class="activity-status ${status}">${label}</span></span>`,
      );
      expect(summary?.match(/<svg\b/g)).toHaveLength(1);
      expect(summary).not.toContain('activity-count');
      expect(summary).not.toContain('activity-failed-count');
      expect(summary).not.toContain('tool call');
    },
  );
});

function toolMarkup(tool: Partial<ToolActivity> & Pick<ToolActivity, 'name'>) {
  return renderToStaticMarkup(
    <ActivityMessage
      content=""
      activity={{
        status: 'running',
        tools: [{ toolCallId: 'file-action', status: 'running', ...tool }],
      }}
    />,
  );
}

function toolHeading(markup: string) {
  return markup.match(
    /<details class="activity-tool [^"]+"><summary>.*?<span class="activity-title">(.*?)<\/span>/,
  )?.[1];
}

describe('file tool row titles', () => {
  it.each([
    { name: 'view', path: '/a/b/c/a.py', expected: 'Viewing file: a.py' },
    { name: 'builtin:view', path: '/a/b/c/a.py', expected: 'Viewing file: a.py' },
    { name: 'read_file', path: 'relative/path/report.md', expected: 'Viewing file: report.md' },
    { name: 'edit', path: '/a/b/c/a.py', expected: 'Editing file: a.py' },
    { name: 'builtin:edit', path: 'C:\\project\\src\\main.ts', expected: 'Editing file: main.ts' },
    {
      name: 'edit_file',
      path: '\\\\server\\share\\src\\main.ts',
      expected: 'Editing file: main.ts',
    },
    { name: 'view', path: 'C:/project\\src/main.ts', expected: 'Viewing file: main.ts' },
    { name: 'edit', path: 'my file.py', expected: 'Editing file: my file.py' },
  ])('shows only the basename for $name $path', ({ name, path, expected }) => {
    const markup = toolMarkup({ name, title: 'Generic runtime title', arguments: { path } });
    expect(toolHeading(markup)).toBe(expected);
    expect(markup).toContain('Arguments');
    expect(markup).toContain(path.replaceAll('\\', '\\\\'));
    expect(markup).not.toContain('Generic runtime title');
  });

  it.each([
    { command: 'view', expected: 'Viewing file: a.py' },
    { command: 'str_replace', expected: 'Editing file: a.py' },
    { command: 'insert', expected: 'Editing file: a.py' },
    { command: 'undo_edit', expected: 'Editing file: a.py' },
  ])('uses the universal editor’s $command action', ({ command, expected }) => {
    expect(
      toolHeading(
        toolMarkup({ name: 'str_replace_editor', arguments: { command, path: '/a/b/a.py' } }),
      ),
    ).toBe(expected);
  });

  it.each([
    undefined,
    null,
    '/a/b/a.py',
    [],
    { path: 42 },
    { path: {} },
    { path: '' },
    { path: '   ' },
    { path: '/a/b/' },
    { path: '/' },
    { path: '.' },
    { path: '..' },
    { fileName: '/a/b/a.py' },
  ])('keeps the existing title for missing or malformed arguments %j', (args) => {
    const markup = toolMarkup({
      name: 'view',
      title: 'Read source',
      ...(args === undefined ? {} : { arguments: args }),
    });
    expect(toolHeading(markup)).toBe('Read source');
  });

  it.each(['bash', 'grep', 'create', 'apply_patch', 'mcp_reader/view'])(
    'does not infer a file action for unrelated %s tools',
    (name) => {
      expect(
        toolHeading(
          toolMarkup({ name, title: 'Existing title', arguments: { path: '/a/b/a.py' } }),
        ),
      ).toBe('Existing title');
    },
  );

  it('keeps the universal editor’s title when its action is missing or unknown', () => {
    for (const args of [{ path: '/a/b/a.py' }, { path: '/a/b/a.py', command: 'create' }])
      expect(
        toolHeading(
          toolMarkup({ name: 'str_replace_editor', title: 'Editor action', arguments: args }),
        ),
      ).toBe('Editor action');
  });

  it('translates file action titles and escapes filenames', () => {
    setLocale('zh-CN');
    expect(toolHeading(toolMarkup({ name: 'view', arguments: { path: '/a/b/a.py' } }))).toBe(
      '查看文件：a.py',
    );
    expect(toolHeading(toolMarkup({ name: 'edit', arguments: { path: '/a/b/<a>.py' } }))).toBe(
      '编辑文件：&lt;a&gt;.py',
    );
  });
});
