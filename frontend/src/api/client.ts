import type { FileNodeResponse, IssueBundlesResponse, LogSearchResponse, UploadResponse } from './types';

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8080';

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
  fetchIssueBundles(issueId: string) {
    return request<IssueBundlesResponse>(`/api/issues/${issueId}`);
  },
  fetchFileNode(bundleId: string, fileId: string) {
    return request<FileNodeResponse>(`/api/files/v1/${bundleId}/files/${fileId}`);
  },
  searchLogs(bundleId: string, query: string, timeline?: string) {
    const params = new URLSearchParams({ q: query });
    if (timeline) params.set('timeline', timeline);
    return request<LogSearchResponse>(`/api/log/v2/${bundleId}/search?${params.toString()}`);
  },
  uploadLogs(issueCode: string, files: File[], bundleName?: string) {
    const formData = new FormData();
    formData.append('issue_code', issueCode);
    if (bundleName) {
      formData.append('bundle_name', bundleName);
    }
    files.forEach((file) => formData.append('files', file, file.name));
    return request<UploadResponse>(`/api/uploads`, {
      method: 'POST',
      body: formData
    });
  }
};
