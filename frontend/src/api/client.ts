import type {
  FileContentResponse,
  FileLinesResponse,
  FileNodeResponse,
  IssueBundlesResponse,
  IssueLogSearchResponse,
  IssueSummary,
  LogSearchResponse,
  UploadResponse,
  UploadTaskResponse
} from './types';

const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL || window.location.origin).replace(/\/$/, '');

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

  const text = await response.text();

  if (!response.ok) {
    throw new Error(text || `Request failed: ${response.status}`);
  }

  if (!text) {
    return undefined as T;
  }

  return JSON.parse(text) as T;
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
  fetchFileLines(bundleId: string, fileId: string, options?: { start?: number; limit?: number }) {
    const params = new URLSearchParams();
    if (typeof options?.start === 'number') params.set('start', String(options.start));
    if (typeof options?.limit === 'number') params.set('limit', String(options.limit));
    const query = params.toString();
    return request<FileLinesResponse>(`/api/files/v1/${bundleId}/files/${fileId}/lines${query ? `?${query}` : ''}`);
  },
  fileDownloadUrl(bundleId: string, fileId: string) {
    return `${API_BASE_URL}/api/files/v1/${bundleId}/files/${fileId}/download`;
  },
  deleteFile(bundleId: string, fileId: string) {
    return request<void>(`/api/files/v1/${bundleId}/files/${fileId}`, { method: 'DELETE' });
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
  searchIssueLogs(issueCode: string, query: string, options?: { path_like?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.path_like) params.set('path_like', options.path_like);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<IssueLogSearchResponse>(`/api/issues/${issueCode}/search?${params.toString()}`);
  },
  fetchUploadTask(taskId: string) {
    return request<UploadTaskResponse>(`/api/uploads/${taskId}`);
  },
  uploadLogs(issueCode: string, files: File[], onProgress?: (percent: number) => void) {
    const formData = new FormData();
    formData.append('issue_code', issueCode);
    files.forEach((file) => formData.append('files', file, file.name));

    if (!onProgress) {
      return request<UploadResponse>(`/api/uploads`, {
        method: 'POST',
        body: formData
      });
    }

    return new Promise<UploadResponse>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('POST', `${API_BASE_URL}/api/uploads`);
      xhr.setRequestHeader('Accept', 'application/json');
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          onProgress(Math.round((event.loaded / event.total) * 100));
        }
      };
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          resolve(JSON.parse(xhr.responseText) as UploadResponse);
          return;
        }
        reject(new Error(xhr.responseText || `Request failed: ${xhr.status}`));
      };
      xhr.onerror = () => reject(new Error('upload failed'));
      xhr.send(formData);
    });
  }
};
