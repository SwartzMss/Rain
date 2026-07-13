export type UploadStatus = 'READY' | 'PROCESSING' | 'FAILED' | 'PENDING';
export type UploadStage = 'PENDING' | 'EXTRACTING' | 'INDEXING' | 'READY' | 'FAILED';

export interface UploadSummary {
  hash: string;
  name: string;
  status: {
    upload_status: UploadStatus;
    [key: string]: unknown;
  };
  stage: UploadStage;
  size_bytes?: number | null;
}

export interface IssueBundlesResponse {
  name: string;
  log_bundles: UploadSummary[];
}

export interface IssueSummary {
  code: string;
  name: string;
  bundle_count: number;
}

export interface CreateIssueRequest {
  code: string;
  name?: string;
}

export interface FileNode {
  id: number | string;
  name: string;
  path: string;
  is_dir: boolean;
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
}
