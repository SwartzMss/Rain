import type {
  FileContentResponse,
  FileNodeResponse,
  IssueBundlesResponse,
  IssueSummary,
  LogSearchResponse,
  UploadResponse
} from './types';

const API_BASE_URL = window.location.origin;

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const isFormData = typeof FormData !== 'undefined' && init?.body instanceof FormData;
  const headers = new Headers(init?.headers as HeadersInit);

  if (!headers.has('Accept')) {
    headers.set('Accept', 'application/json');
  }

  if (!isFormData && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  const response = await fetch(`${API_BASE_URL}${path}`, {
    headers,
    ...init
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `Request failed: ${response.status}`);
  }

  return response.json() as Promise<T>;
}

export const rainApi = {
  fetchIssues() {
    return request<IssueSummary[]>(`/api/issues`);
  },
  fetchIssueBundles(issueId: string) {
    return request<IssueBundlesResponse>(`/api/issues/${issueId}`);
  },
  fetchFileNode(bundleId: string, fileId: string) {
    return request<FileNodeResponse>(`/api/files/v1/${bundleId}/files/${fileId}`);
  },
  fetchFileContent(bundleId: string, fileId: string) {
    return request<FileContentResponse>(`/api/files/v1/${bundleId}/files/${fileId}/content`);
  },
  deleteBundle(issueCode: string, bundleHash: string) {
    return request<void>(`/api/issues/${issueCode}/bundles/${bundleHash}`, { method: 'DELETE' });
  },
  deleteIssue(issueCode: string) {
    return request<void>(`/api/issues/${issueCode}`, { method: 'DELETE' });
  },
  searchLogs(bundleId: string, query: string, options?: { timeline?: string; path_like?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.timeline) params.set('timeline', options.timeline);
    if (options?.path_like) params.set('path_like', options.path_like);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<LogSearchResponse>(`/api/log/v2/${bundleId}/search?${params.toString()}`);
  },
  uploadLogs(issueCode: string, files: File[]) {
    const formData = new FormData();
    formData.append('issue_code', issueCode);
    files.forEach((file) => formData.append('files', file, file.name));
    return request<UploadResponse>(`/api/uploads`, {
      method: 'POST',
      body: formData
    });
  }
};
