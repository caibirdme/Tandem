export type ProviderProtocol = 'openai-completions' | 'openai-responses' | 'anthropic';
export type ReasoningEffort = 'low' | 'medium' | 'high' | null;
export type FontSize = 'small' | 'medium' | 'large';
export interface Model {
  id: string;
  name: string;
  reasoningEffort: ReasoningEffort;
  contextWindow: number;
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
  fontSize: FontSize;
}
export type Connection =
  | { kind: 'local' }
  | {
      kind: 'ssh';
      host: string;
      port: number | null;
      user: string | null;
      identityFile: string | null;
    };
export interface Project {
  id: string;
  name: string;
  path: string;
  connection: Connection;
  createdAt: string;
}
export interface SelectedSkill {
  name: string;
  path: string;
}
export interface ProjectSkill extends SelectedSkill {
  description: string;
}
export type SessionStatus = 'idle' | 'running' | 'error';
export type PermissionMode = 'ask' | 'allow-all';
export interface ContextUsage {
  currentTokens: number;
  tokenLimit: number;
}
export interface AttachmentInput {
  name: string;
  mimeType: string;
  data: string;
}
export interface Attachment {
  id: string;
  name: string;
  mimeType: string;
  size: number;
}
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
  reasoningEffort: ReasoningEffort;
  contextUsage: ContextUsage | null;
}
export interface Message {
  id: string;
  sessionId: string;
  role: 'user' | 'assistant' | 'activity' | 'reasoning' | 'tool' | 'system' | 'agent';
  agentId?: string;
  agent?: Subagent;
  fleet?: boolean;
  interrupted?: boolean;
  content: string;
  createdAt: string;
  selectedSkills?: SelectedSkill[];
  attachments: Attachment[];
  activity?: Activity;
  reasoning?: { status: ActivityStatus };
  sequence?: number;
}
export interface RuntimeStatus {
  available: boolean;
  version: string | null;
  message: string;
}
export interface Snapshot {
  titleGenerations: string[];
  activeTurns: Record<
    string,
    { stream: string; agentStreams: Record<string, string>; permissions: Permission[] }
  >;
  settings: Settings;
  projects: Project[];
  sessions: Session[];
  runtime: RuntimeStatus;
}
export type ActivityStatus = 'running' | 'completed' | 'error' | 'interrupted';
export interface Subagent {
  id: string;
  toolCallId: string;
  name: string;
  displayName: string;
  description: string;
  status: ActivityStatus;
  model?: string;
  error?: string;
  parentAgentId?: string;
}
export interface Activity {
  status: ActivityStatus;
  tools: ToolActivity[];
}
export interface ToolActivity {
  toolCallId: string;
  name: string;
  title?: string;
  status: ActivityStatus;
  arguments?: unknown;
  result?: unknown;
  error?: unknown;
  progress?: string;
}
export interface Permission {
  requestId: string;
  kind: string;
  description: string;
}
export type RuntimeEvent = { sessionId: string } & (
  | { kind: 'delta'; data: { content: string; agentId?: string } }
  | { kind: 'message'; data: Message }
  | { kind: 'permission'; data: Permission }
  | { kind: 'status'; data: { status: SessionStatus } }
  | { kind: 'error'; data: { message: string } }
  | { kind: 'title'; data: { title: string } }
  | { kind: 'title_error'; data: { message: string } }
  | { kind: 'context_usage'; data: ContextUsage | null }
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
  save_font_size: { payload: { fontSize: FontSize }; result: null };
  save_title_model: { payload: { titleModel: ModelSelection | null }; result: null };
  regenerate_title: { payload: { sessionId: string }; result: null };
  create_project: {
    payload: { name: string; path: string; connection: Connection };
    result: Project;
  };
  delete_project: { payload: { id: string }; result: null };
  list_project_skills: { payload: { projectId: string }; result: ProjectSkill[] };
  create_session: {
    payload: {
      projectId: string | null;
      providerId: string;
      modelId: string;
      permissionMode?: PermissionMode;
      reasoningEffort?: ReasoningEffort;
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
  set_session_reasoning_effort: {
    payload: { sessionId: string; reasoningEffort: ReasoningEffort };
    result: Session;
  };
  delete_session: { payload: { sessionId: string }; result: null };
  rename_session: { payload: { sessionId: string; title: string }; result: null };
  messages: { payload: { sessionId: string }; result: Message[] };
  get_attachment: {
    payload: { sessionId: string; attachmentId: string };
    result: AttachmentInput;
  };
  send_message: {
    payload: {
      sessionId: string;
      content: string;
      messageId: string;
      selectedSkills?: SelectedSkill[];
      attachments?: AttachmentInput[];
      fleet?: boolean;
    };
    result: null;
  };
  abort_session: { payload: { sessionId: string }; result: null };
  respond_permission: {
    payload: { sessionId: string; requestId: string; approved: boolean };
    result: null;
  };
  check_connection: { payload: { connection: Connection }; result: { message: string } };
  list_ssh_hosts: { payload: Record<string, never>; result: string[] };
  browse_directory: {
    payload: { connection: Connection; path: string };
    result: DirectoryListing;
  };
  install_runtime: { payload: Record<string, never>; result: RuntimeStatus };
}
