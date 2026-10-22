export type ReviewSelection =
  | { kind: 'workspace'; comparison: 'net' | 'staged' | 'unstaged' }
  | { kind: 'session' }
  | { kind: 'turn'; messageId: string };

export interface ReviewSummary {
  reviewId: string;
  revision: string;
  workspacePath: string;
  repositoryPath: string;
  selection: ReviewSelection;
  capturedAt: number;
  filesTotal: number;
  additions: number | null;
  deletions: number | null;
  complete: boolean;
  excludedFiles: number;
}

export type ReviewUnavailableCode =
  | 'not_repository'
  | 'git_unavailable'
  | 'remote_unavailable'
  | 'baseline_unavailable'
  | 'capturing'
  | 'inspection_failed';

export interface ReviewOverview {
  summary: ReviewSummary | null;
  unavailable: { code: ReviewUnavailableCode; message: string } | null;
  turns: { messageId: string; capturedAt: number; complete: boolean; available: boolean }[];
}

export interface ReviewFile {
  id: string;
  path: string;
  oldPath: string | null;
  oldMode: string | null;
  newMode: string | null;
  status: 'added' | 'modified' | 'deleted' | 'renamed' | 'type_changed' | 'conflict' | 'unchanged';
  indexStatus: string | null;
  worktreeStatus: string | null;
  contentKind: 'text' | 'binary' | 'too_large' | 'unsupported' | 'conflict';
  additions: number | null;
  deletions: number | null;
}

export interface ReviewFilePage {
  files: ReviewFile[];
  nextCursor: number | null;
}

export interface ReviewLine {
  kind: 'context' | 'addition' | 'deletion' | 'note';
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

export interface ReviewHunk {
  oldStart: number;
  newStart: number;
  header: string;
  lines: ReviewLine[];
}

export interface ReviewFileDiff {
  reviewId: string;
  revision: string;
  file: ReviewFile;
  patch: string | null;
  hunks: ReviewHunk[];
}
