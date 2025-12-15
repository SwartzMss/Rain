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
}

export interface LogSearchResponse {
  total: number;
  hits: LogSearchHit[];
}

export interface UploadResponse {
  issue_code: string;
  bundle_hash: string;
  bundle_name: string;
  file_count: number;
  total_bytes: number;
}
