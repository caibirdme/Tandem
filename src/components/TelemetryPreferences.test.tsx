import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';

import * as telemetryCredential from '../hooks/useTelemetryCredential';
import { setLocale } from '../i18n';
import { initialCredentialEditor } from '../lib/credentialEditor';
import type { Settings } from '../lib/types';
import { Preferences } from './Settings';

const settings: Settings = {
  titleModel: null,
  cliPath: null,
  fontSize: 'medium',
  providers: [],
  telemetry: {
    enabled: true,
    endpoint: 'https://collector.example.com:4318',
    protocol: 'http/json',
    captureContent: true,
  },
};

describe('telemetry destination settings', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    setLocale('en');
  });

  it.each([
    ['en', 'Enter your AgentLens workspace access key'],
    ['zh-CN', '请输入 AgentLens 空间的接入密钥'],
  ] as const)(
    'requests the workspace access key when no key is saved in %s',
    (locale, placeholder) => {
      setLocale(locale);
      vi.spyOn(telemetryCredential, 'useTelemetryCredential').mockReturnValue({
        state: initialCredentialEditor(false),
        hasSavedKey: false,
        edit: { kind: 'keep' },
        update: () => undefined,
        retry: () => undefined,
        saved: () => undefined,
      });
      const rendered = renderToStaticMarkup(
        <Preferences
          mode="telemetry"
          settings={{
            ...settings,
            telemetry: { ...settings.telemetry, agentlens: { serviceName: 'tandem' } },
          }}
          onSaved={async () => undefined}
        />,
      );
      expect(rendered).toContain(`placeholder="${placeholder}"`);
      expect(rendered).not.toContain('optional for local services');
      expect(rendered).not.toContain('本地服务可留空');
    },
  );

  it('shows the AgentLens key masked and states exactly which content and sessions are exported', () => {
    const rendered = renderToStaticMarkup(
      <Preferences
        mode="telemetry"
        settings={{
          ...settings,
          telemetry: { ...settings.telemetry, agentlens: { serviceName: 'tandem' } },
        }}
        onSaved={async () => undefined}
      />,
    );
    expect(rendered).toContain('AgentLens access key');
    expect(rendered).toContain('type="password"');
    expect(rendered).toContain('value="tandem"');
    expect(rendered).toContain('system instructions, and tool inputs and outputs');
    expect(rendered).toContain(
      'Conversation title generation and other background requests are excluded',
    );
    expect(rendered).not.toContain('Transport protocol');
  });

  it('keeps generic OTLP protocol selection and excludes the AgentLens credential editor', () => {
    const rendered = renderToStaticMarkup(
      <Preferences mode="telemetry" settings={settings} onSaved={async () => undefined} />,
    );
    expect(rendered).toContain('Transport protocol');
    expect(rendered).toContain('HTTP / Protobuf');
    expect(rendered).not.toContain('AgentLens access key');
    expect(rendered).not.toContain('type="password"');
  });
});
