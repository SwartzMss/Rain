export type UploadStatus = 'READY' | 'PROCESSING' | 'FAILED' | 'PENDING';
export type UploadStage = 'PENDING' | 'RECEIVING' | 'EXTRACTING' | 'INDEXING' | 'PUBLISHING' | 'READY' | 'FAILED';

export type UserRole = 'USER' | 'ADMIN';
export type UserStatus = 'ACTIVE' | 'DISABLED';

export interface User {
  id: string;
  username: string;
  role: UserRole;
}

export interface AdminUser { id: string; username: string; status: UserStatus; created_at: string; updated_at: string; last_login_at: string | null; active_session_count: number; issue_count: number; storage_bytes: number; }
export interface AdminUserPage { items: AdminUser[]; next_cursor: string | null; }
export interface AuditLog { id: string; actor_type: 'USER' | 'SYSTEM'; actor_user_id: string | null; target_user_id: string | null; target_username: string | null; action: string; old_value: string | null; new_value: string | null; client_ip: string | null; user_agent?: string | null; created_at: string; }
export interface AuditLogPage { items: AuditLog[]; next_cursor: string | null; }

export interface Credentials {
  username: string;
  password: string;
}

export interface AuthMeResponse {
  authenticated: boolean;
  user: User | null;
}

export interface SavedSearchPayload {
  name: string;
  search_type: 'FILENAME' | 'DETAIL';
  query_text: string;
  options: Record<string, unknown>;
  is_pinned?: boolean;
}

export interface SavedSearch extends SavedSearchPayload {
  id: string;
  is_pinned: boolean;
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
}

export interface UploadSummary {
  hash: string;
  name: string;
  status: {
    upload_status: UploadStatus;
    [key: string]: unknown;
  };
  stage: UploadStage;
  failure_reason?: string | null;
  failure_stage?: string | null;
  failure_code?: string | null;
  retryable?: boolean | null;
  size_bytes?: number | null;
}

export interface IssueBundlesResponse {
  name: string;
  can_write: boolean;
  owner_username: string | null;
  log_bundles: UploadSummary[];
}

export interface IssueSummary {
  code: string;
  name: string;
  bundle_count: number;
  can_write: boolean;
  owner_username: string | null;
}

export interface CreateIssueRequest {
  code: string;
  name?: string;
}

export interface FileNode {
  id: number | string;
  parent_id?: number | string | null;
  name: string;
  path: string;
  is_dir: boolean;
  preview_kind: 'directory' | 'text' | 'binary' | 'archive';
  size_bytes?: number;
  mime_type?: string;
  status?: string;
  children?: FileNode[];
  meta?: Record<string, unknown>;
}

export interface FileNodeResponse {
  node: FileNode;
  children?: FileNode[];
}

export interface LogSearchHit {
  file_id: number | string;
  path: string;
  snippet: string;
  timeline?: string;
  offset?: number;
  line_number?: number;
  chunk_index?: number;
}

export interface LogSearchResponse {
  total: number;
  hits: LogSearchHit[];
  truncated: boolean;
}

export interface UploadResponse {
  task_id: string;
  issue_code: string;
  bundle_hash: string;
  status: UploadStatus;
  stage: UploadStage;
  file_count: number;
  total_bytes: number;
}

export interface UploadTaskResponse {
  task_id: string;
  issue_code: string;
  bundle_hash: string;
  status: UploadStatus;
  stage: UploadStage;
  failure_reason?: string | null;
  failure_stage?: string | null;
  failure_code?: string | null;
  retryable?: boolean | null;
  progress_percent: number;
  total_bytes: number;
}

export interface FileContentResponse {
  path: string;
  size_bytes?: number;
  mime_type?: string;
  preview: string;
  truncated: boolean;
}

export interface FileLine {
  line_number: number;
  content: string;
  truncated?: boolean;
}

export interface FileLinesResponse {
  path: string;
  size_bytes?: number;
  line_count?: number | null;
  start: number;
  limit: number;
  next_start?: number | null;
  lines: FileLine[];
}

export interface IssueLogSearchHit {
  file_id: string | number;
  path: string;
  bundle_hash?: string;
  snippet: string;
  timeline?: string;
  line_end?: number | null;
  line_number?: number | null;
}

export interface IssueLogSearchResponse {
  total: number;
  hits: IssueLogSearchHit[];
  truncated: boolean;
}

export interface TempResultInfo {
  id: string;
  name: string;
  expression: string;
  source_label: string;
  line_count: number;
  size_bytes: number;
  created_at: string;
  expires_at: string;
}

export interface TempResultLinesResponse {
  start: number;
  limit: number;
  line_count: number;
  next_start?: number | null;
  lines: Array<FileLine & {
    bundle_hash?: string | null;
    file_id?: string | null;
    path?: string | null;
  }>;
}

export interface TempResultPreviewResponse {
  result_id: string;
  total: number;
  lines: Array<FileLine & {
    bundle_hash?: string;
    file_id?: string;
    path: string;
  }>;
}
