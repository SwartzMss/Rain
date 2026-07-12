import type {
  FileContentResponse,
  FileLinesResponse,
  FileNodeResponse,
  CreateIssueRequest,
  IssueBundlesResponse,
  IssueLogSearchResponse,
  IssueSummary,
  LogSearchResponse,
  UploadResponse,
  UploadTaskResponse
} from './types';

const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL || window.location.origin).replace(/\/$/, '');
const ISSUE_CODE_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

export function normalizeApiError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error || '');

  if (/failed to fetch|networkerror|upload failed/i.test(message)) {
    return '无法连接 Rain 后端，请确认服务已启动';
  }

  return message || '请求失败';
}

export function normalizeIssueCode(value: string): string {
  const code = value.trim().toUpperCase();
  if (!ISSUE_CODE_PATTERN.test(code)) {
    throw new Error("Issue ID 只能包含字母、数字、'.'、'_'、'-'，长度 1-64");
  }
  return code;
}

const encodePathSegment = (value: string) => encodeURIComponent(value);

function parseErrorResponse(text: string, status: number): string {
  let message = text;
  try {
    const payload = JSON.parse(text) as { error?: unknown };
    if (typeof payload.error === 'string' && payload.error.trim()) {
      message = payload.error;
    }
  } catch {
    // Keep the original response text when it is not JSON.
  }

  return message || `请求失败：${status}`;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const isFormData = typeof FormData !== 'undefined' && init?.body instanceof FormData;
  const headers = new Headers(init?.headers as HeadersInit);

  if (!headers.has('Accept')) {
    headers.set('Accept', 'application/json');
  }

  if (!isFormData && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  let response: Response;
  try {
    response = await fetch(`${API_BASE_URL}${path}`, {
      headers,
      ...init
    });
  } catch (error) {
    throw new Error(normalizeApiError(error));
  }

  const text = await response.text();

  if (!response.ok) {
    throw new Error(parseErrorResponse(text, response.status));
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
  createIssue(payload: CreateIssueRequest) {
    return request<IssueSummary>('/api/issues', {
      method: 'POST',
      body: JSON.stringify({
        code: normalizeIssueCode(payload.code),
        name: payload.name?.trim() || undefined
      })
    });
  },
  fetchIssueBundles(issueId: string) {
    return request<IssueBundlesResponse>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueId))}`);
  },
  fetchFileNode(bundleId: string, fileId: string) {
    return request<FileNodeResponse>(`/api/files/v1/${encodePathSegment(bundleId)}/files/${encodePathSegment(fileId)}`);
  },
  fetchFileContent(bundleId: string, fileId: string) {
    return request<FileContentResponse>(`/api/files/v1/${encodePathSegment(bundleId)}/files/${encodePathSegment(fileId)}/content`);
  },
  fetchFileLines(bundleId: string, fileId: string, options?: { start?: number; limit?: number }) {
    const params = new URLSearchParams();
    if (typeof options?.start === 'number') params.set('start', String(options.start));
    if (typeof options?.limit === 'number') params.set('limit', String(options.limit));
    const query = params.toString();
    return request<FileLinesResponse>(`/api/files/v1/${encodePathSegment(bundleId)}/files/${encodePathSegment(fileId)}/lines${query ? `?${query}` : ''}`);
  },
  fileDownloadUrl(bundleId: string, fileId: string) {
    return `${API_BASE_URL}/api/files/v1/${encodePathSegment(bundleId)}/files/${encodePathSegment(fileId)}/download`;
  },
  deleteFile(bundleId: string, fileId: string) {
    return request<void>(`/api/files/v1/${encodePathSegment(bundleId)}/files/${encodePathSegment(fileId)}`, { method: 'DELETE' });
  },
  deleteBundle(issueCode: string, bundleHash: string) {
    return request<void>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueCode))}/bundles/${encodePathSegment(bundleHash)}`, { method: 'DELETE' });
  },
  deleteIssue(issueCode: string) {
    return request<void>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueCode))}`, { method: 'DELETE' });
  },
  searchLogs(bundleId: string, query: string, options?: { timeline?: string; path_like?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.timeline) params.set('timeline', options.timeline);
    if (options?.path_like) params.set('path_like', options.path_like);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<LogSearchResponse>(`/api/log/v2/${encodePathSegment(bundleId)}/search?${params.toString()}`);
  },
  searchIssueLogs(issueCode: string, query: string, options?: { path_like?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.path_like) params.set('path_like', options.path_like);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<IssueLogSearchResponse>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueCode))}/search?${params.toString()}`);
  },
  fetchUploadTask(taskId: string) {
    return request<UploadTaskResponse>(`/api/uploads/${encodePathSegment(taskId)}`);
  },
  uploadLogs(issueCode: string, files: File[], onProgress?: (percent: number) => void) {
    const normalizedIssueCode = normalizeIssueCode(issueCode);
    const formData = new FormData();
    formData.append('issue_code', normalizedIssueCode);
    files.forEach((file) => formData.append('files', file, file.name));
    const path = `/api/issues/${encodePathSegment(normalizedIssueCode)}/uploads`;

    if (!onProgress) {
      return request<UploadResponse>(path, {
        method: 'POST',
        body: formData
      });
    }

    return new Promise<UploadResponse>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('POST', `${API_BASE_URL}${path}`);
      xhr.timeout = 30 * 60 * 1000;
      xhr.setRequestHeader('Accept', 'application/json');
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          onProgress(Math.round((event.loaded / event.total) * 100));
        }
      };
      xhr.onload = () => {
        if (xhr.status < 200 || xhr.status >= 300) {
          reject(new Error(parseErrorResponse(xhr.responseText, xhr.status)));
          return;
        }

        try {
          resolve(JSON.parse(xhr.responseText) as UploadResponse);
        } catch {
          reject(new Error('服务器返回了无法解析的上传响应'));
        }
      };
      xhr.onerror = () => reject(new Error(normalizeApiError(new Error('upload failed'))));
      xhr.ontimeout = () => reject(new Error('上传超时'));
      xhr.onabort = () => reject(new Error('上传已取消'));
      xhr.send(formData);
    });
  }
};
