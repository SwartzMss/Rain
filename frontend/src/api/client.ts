import type { FileNodeResponse, IssueBundlesResponse, LogSearchResponse } from './types';

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8080';

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE_URL}${path}`, {
    headers: {
      Accept: 'application/json',
      'Content-Type': 'application/json',
      ...(init?.headers ?? {})
    },
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
  }
};
