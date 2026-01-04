export type UploadStatus = 'READY' | 'PROCESSING' | 'FAILED' | 'PENDING';

export interface UploadSummary {
  hash: string;
  name: string;
  status: {
    upload_status: UploadStatus;
    [key: string]: unknown;
  };
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
  issue_code: string;
  bundle_hash: string;
  file_count: number;
  total_bytes: number;
}

export interface FileContentResponse {
  path: string;
  size_bytes?: number;
  mime_type?: string;
  preview: string;
  truncated: boolean;
}

export interface IssueLogSearchHit {
  file_id: string | number;
  path: string;
  bundle_hash?: string;
  snippet: string;
  line_end?: number | null;
  line_number?: number | null;
}

export interface IssueLogSearchResponse {
  total: number;
  hits: IssueLogSearchHit[];
}
