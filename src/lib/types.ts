export type ProviderProtocol = 'openai-completions' | 'openai-responses' | 'anthropic';
export interface Model {
  id: string;
  name: string;
  reasoningEffort: 'low' | 'medium' | 'high' | null;
  contextWindow: number | null;
  maxOutputTokens: number | null;
}
export interface ModelSelection {
  providerId: string;
  modelId: string;
}
export interface Provider {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string;
  hasApiKey: boolean;
  models: Model[];
}
export interface Telemetry {
  enabled: boolean;
  endpoint: string;
  protocol: 'http/protobuf' | 'http/json';
  captureContent: boolean;
}
export interface Settings {
  providers: Provider[];
  titleModel: ModelSelection | null;
  telemetry: Telemetry;
  cliPath: string | null;
}
export type Connection =
  | { kind: 'local' }
  | { kind: 'ssh'; host: string; port: number; user: string; identityFile: string | null };
export interface Project {
  id: string;
  name: string;
  path: string;
  connection: Connection;
  createdAt: string;
}
export type SessionStatus = 'idle' | 'running' | 'error';
export type PermissionMode = 'ask' | 'allow-all';
export interface Session {
  titleState: 'preserved' | 'pending' | 'attempted';
  id: string;
  title: string;
  projectId: string | null;
  providerId: string;
  modelId: string;
  createdAt: string;
  updatedAt: string;
  status: SessionStatus;
  permissionMode: PermissionMode;
}
export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'tool' | 'system';
  interrupted?: boolean;
  content: string;
  createdAt: string;
}
export interface RuntimeStatus {
  available: boolean;
  version: string | null;
  message: string;
}
export interface Snapshot {
  titleGenerations: string[];
  activeTurns: Record<string, { stream: string; permissions: Permission[] }>;
  settings: Settings;
  projects: Project[];
  sessions: Session[];
  runtime: RuntimeStatus;
}
export interface ToolActivity {
  toolCallId?: string;
  name: string;
  status: 'running' | 'completed' | 'error';
  content: string;
}
export interface Permission {
  requestId: string;
  kind: string;
  description: string;
}
export type RuntimeEvent = { sessionId: string } & (
  | { kind: 'delta'; data: { content: string } }
  | { kind: 'message'; data: Message }
  | { kind: 'tool'; data: ToolActivity }
  | { kind: 'permission'; data: Permission }
  | { kind: 'status'; data: { status: SessionStatus } }
  | { kind: 'error'; data: { message: string } }
  | { kind: 'title'; data: { title: string } }
  | { kind: 'title_error'; data: { message: string } }
);
export interface DirectoryListing {
  path: string;
  entries: { name: string; path: string }[];
}
export type ProviderDraft = Omit<Provider, 'hasApiKey'>;
export interface Commands {
  snapshot: { payload: Record<string, never>; result: Snapshot };
  save_provider: { payload: { provider: ProviderDraft; apiKey?: string }; result: Provider };
  get_provider_api_key: { payload: { id: string }; result: string | null };
  delete_provider: { payload: { id: string }; result: null };
  save_preferences: { payload: { telemetry: Telemetry; cliPath: string | null }; result: null };
  save_title_model: { payload: { titleModel: ModelSelection | null }; result: null };
  regenerate_title: { payload: { sessionId: string }; result: null };
  create_project: {
    payload: { name: string; path: string; connection: Connection };
    result: Project;
  };
  delete_project: { payload: { id: string }; result: null };
  create_session: {
    payload: {
      projectId: string | null;
      providerId: string;
      modelId: string;
      permissionMode?: PermissionMode;
    };
    result: Session;
  };
  set_permission_mode: {
    payload: { sessionId: string; permissionMode: PermissionMode };
    result: Session;
  };
  set_session_model: {
    payload: { sessionId: string; providerId: string; modelId: string };
    result: Session;
  };
  delete_session: { payload: { sessionId: string }; result: null };
  rename_session: { payload: { sessionId: string; title: string }; result: null };
  messages: { payload: { sessionId: string }; result: Message[] };
  send_message: {
    payload: { sessionId: string; content: string; messageId: string };
    result: null;
  };
  abort_session: { payload: { sessionId: string }; result: null };
  respond_permission: {
    payload: { sessionId: string; requestId: string; approved: boolean };
    result: null;
  };
  check_connection: { payload: { connection: Connection }; result: { message: string } };
  browse_directory: {
    payload: { connection: Connection; path: string };
    result: DirectoryListing;
  };
  install_runtime: { payload: Record<string, never>; result: RuntimeStatus };
}
