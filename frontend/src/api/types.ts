export type UploadStatus = 'READY' | 'PROCESSING' | 'FAILED' | 'PENDING';

export type UploadStage = 'PENDING' | 'RECEIVING' | 'VALIDATING' | 'EXTRACTING' | 'INDEXING' | 'PUBLISHING' | 'READY' | 'FAILED';

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
export interface RegistrationStatus { allow_registration: boolean; }
export type SettingCategory = 'common' | 'advanced' | 'expert';
export type SettingVisibility = 'default' | 'collapsed' | 'expert';
export type ResourceMode = 'auto' | 'manual';

export interface AdminSecurityStatus {
  argon2id_enabled: boolean;
}

export interface RegistrationSettingField {
  key: string;
  db_column: string;
  env_name: string;
  value_type?: string;
  unit?: string | null;
  default_value?: unknown;
  default_rule?: string | null;
  min?: number | null;
  max?: number | null;
  description?: string;
  apply_mode: 'hot' | 'restart_required';
  sensitive?: boolean;
  category: SettingCategory;
  visibility: SettingVisibility;
  recommended_min?: number | null;
  recommended_max?: number | null;
  supports_auto?: boolean;
  auto_value?: number | null;
  protected?: boolean;
}

export interface RegistrationSettings extends RegistrationStatus {
  schema_version?: number;
  revision?: string;
  updated_at: string;
  updated_by_username: string | null;
  login_ip_limit_per_minute: number;
  login_username_failure_limit_per_5_minutes: number;
  issue_inactive_days: number;
  cleanup_exempt_usernames: string[];
  configured?: Record<string, unknown>;
  effective?: Record<string, unknown>;
  resource_modes?: Record<string, ResourceMode>;
  auto_values?: Record<string, number>;
  security?: AdminSecurityStatus;
  restart_required?: boolean;
  pending_restart_fields?: string[];
  fields?: RegistrationSettingField[];
}
export interface AuthRateLimitEntry { key: string; username: string | null; ip: string | null; current_count: number; limit: number; window_seconds: number; last_event_at: string | null; retry_after_seconds: number; limited: boolean; }
export interface AuthRateLimitsResponse { username_failures: AuthRateLimitEntry[]; login_ips: AuthRateLimitEntry[]; }

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
  inactivity_expiry: IssueInactivityExpiry | null;
  log_bundles: UploadSummary[];
}

export interface IssueInactivityExpiry {
  inactive_days: number;
  expires_at: string;
  renewed_from_expiring: boolean;
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

export interface FileDeletionJobResponse {
  job_id: string;
  status: 'QUEUED' | 'RUNNING' | 'RETRY_WAIT' | 'SUCCEEDED' | 'SUPERSEDED';
  phase: 'WALK' | 'OFFSETS' | 'SEGMENTS' | 'REMOVE_NODE' | 'RECONCILE';
  deleted_files: number;
  deleted_offsets: number;
  deleted_segments: number;
  attempts: number;
  next_retry_at: string | null;
  last_error_code: string | null;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
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
  max_search_window: number;
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

export type UploadSessionStatus = 'OPEN' | 'FINALIZING' | 'DELIVERED' | 'CANCELLED' | 'EXPIRED' | 'FAILED';

export interface UploadSessionResponse {
  session_id: string;
  issue_code: string;
  file_name: string;
  file_size_bytes: number;
  chunk_size_bytes: number;
  committed_offset: number;
  next_chunk_index: number;
  status: UploadSessionStatus;
  bundle_id?: string | null;
  failure_code?: string | null;
  failure_reason?: string | null;
  expires_at: string;
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
  max_search_window: number;
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
  next_start?: number | null;
  lines: Array<FileLine & {
    bundle_hash?: string;
    file_id?: string;
    path: string;
  }>;
}
