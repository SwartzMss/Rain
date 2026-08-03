import type {
  FileContentResponse,
  FileLinesResponse,
  FileNodeResponse,
  CreateIssueRequest,
  IssueBundlesResponse,
  IssueLogSearchResponse,
  IssueSummary,
  LogSearchResponse,
  TempResultInfo,
  TempResultLinesResponse,
  TempResultPreviewResponse,
  UploadResponse,
  UploadTaskResponse,
  AuthMeResponse,
  Credentials,
  User,
  SavedSearch,
  SavedSearchPayload
  , AdminUserPage, AuditLogPage, UserStatus, RegistrationStatus, RegistrationSettings, AuthRateLimitsResponse,
  UserSkill, SkillPayload, SkillReview, AiProviderSettings, SkillRun, SkillRunResult
} from './types';

const API_BASE_URL = '';
const ISSUE_CODE_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly code?: string
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

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

function parseErrorResponse(text: string, status: number): ApiError {
  let message = text;
  let code: string | undefined;
  try {
    const payload = JSON.parse(text) as { code?: unknown; error?: unknown; message?: unknown };
    if (typeof payload.code === 'string' && payload.code.trim()) {
      code = payload.code;
    }
    if (typeof payload.message === 'string' && payload.message.trim()) {
      message = payload.message;
    }
    if (typeof payload.error === 'string' && payload.error.trim()) {
      message = payload.error;
    }
  } catch {
    // Keep the original response text when it is not JSON.
  }

  return new ApiError(message || `请求失败：${status}`, status, code);
}

export function shouldRevalidateAuthentication(status: number, text: string): boolean {
  if (status !== 401 && status !== 403) return false;
  try {
    const payload = JSON.parse(text) as { code?: string };
    return (
      (status === 401 && payload.code === 'AUTHENTICATION_REQUIRED') ||
      (status === 403 &&
        (payload.code === 'ACCOUNT_DISABLED' || payload.code === 'ADMIN_REQUIRED'))
    );
  } catch {
    return false;
  }
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
      ...init,
      headers,
      credentials: 'include'
    });
  } catch (error) {
    throw new Error(normalizeApiError(error));
  }

  const text = await response.text();

  if (!response.ok) {
    if (shouldRevalidateAuthentication(response.status, text)) {
      window.dispatchEvent(new Event('rain:authentication-required'));
    }
    throw parseErrorResponse(text, response.status);
  }

  if (!text) {
    return undefined as T;
  }

  return JSON.parse(text) as T;
}

export const rainApi = {
  fetchSkills() { return request<UserSkill[]>('/api/me/skills'); },
  fetchSkill(id: string) { return request<UserSkill>(`/api/me/skills/${encodePathSegment(id)}`); },
  createSkill(payload: SkillPayload) { return request<UserSkill>('/api/me/skills', { method: 'POST', body: JSON.stringify(payload) }); },
  updateSkill(id: string, payload: SkillPayload) { return request<UserSkill>(`/api/me/skills/${encodePathSegment(id)}`, { method: 'PUT', body: JSON.stringify(payload) }); },
  deleteSkill(id: string) { return request<void>(`/api/me/skills/${encodePathSegment(id)}`, { method: 'DELETE' }); },
  reviewSkill(id: string) { return request<SkillReview>(`/api/me/skills/${encodePathSegment(id)}/review`, { method: 'POST' }); },
  fetchAiProvider() { return request<AiProviderSettings>('/api/admin/ai-provider'); },
  updateAiProvider(payload: { base_url: string; api_key?: string; model: string; request_timeout_seconds: number }) { return request<AiProviderSettings>('/api/admin/ai-provider', { method: 'PUT', body: JSON.stringify(payload) }); },
  testAiProvider(payload?: { base_url: string; api_key: string; model: string; request_timeout_seconds: number }) { return request<{ ok: boolean; model: string }>('/api/admin/ai-provider/test', { method: 'POST', body: payload ? JSON.stringify(payload) : undefined }); },
  fetchAiProviderStatus() { return request<{ configured: boolean }>('/api/me/ai-provider-status'); },
  createSkillRun(issueCode: string, skillId: string) { return request<SkillRun>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueCode))}/skill-runs`, { method: 'POST', body: JSON.stringify({ skill_id: skillId }) }); },
  fetchActiveSkillRun() { return request<SkillRun | null>('/api/me/skill-runs/active'); },
  fetchSkillRun(id: string) { return request<SkillRun>(`/api/skill-runs/${encodePathSegment(id)}`); },
  cancelSkillRun(id: string) { return request<SkillRun>(`/api/skill-runs/${encodePathSegment(id)}/cancel`, { method: 'POST' }); },
  fetchSkillRunResult(id: string) { return request<SkillRunResult>(`/api/skill-runs/${encodePathSegment(id)}/result`); },
  skillRunEventsUrl(id: string) { return `/api/skill-runs/${encodePathSegment(id)}/events`; },
  fetchAdminUsers(params: { query?: string; status?: UserStatus; cursor?: string } = {}) {
    const query = new URLSearchParams(Object.entries(params).filter((entry): entry is [string, string] => Boolean(entry[1])));
    return request<AdminUserPage>(`/api/admin/users?${query}`);
  },
  fetchAuditLogs(cursor?: string) { return request<AuditLogPage>(`/api/admin/audit-logs${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`); },
  fetchAdminSettings() { return request<RegistrationSettings>('/api/admin/settings'); },
  updateAdminSettings(allow_registration?: boolean, login_ip_limit_per_minute?: number, login_username_failure_limit_per_5_minutes?: number, issue_inactive_days?: number) { return request<RegistrationSettings>('/api/admin/settings', { method: 'PATCH', body: JSON.stringify({ allow_registration, login_ip_limit_per_minute, login_username_failure_limit_per_5_minutes, issue_inactive_days }) }); },
  fetchAuthRateLimits() { return request<AuthRateLimitsResponse>('/api/admin/auth-rate-limits'); },
  clearAuthRateLimit(type: 'usernames' | 'ips', key: string) { return request<void>(`/api/admin/auth-rate-limits/${type}/${encodePathSegment(key)}`, { method: 'DELETE' }); },
  clearAllAuthRateLimits(type: 'usernames' | 'ips') { return request<void>(`/api/admin/auth-rate-limits/${type}`, { method: 'DELETE' }); },
  changeUserStatus(id: string, status: UserStatus) { return request(`/api/admin/users/${encodePathSegment(id)}/status`, { method: 'PATCH', body: JSON.stringify({ status }) }); },
  revokeUserSessions(id: string) { return request<{ revoked_sessions: number }>(`/api/admin/users/${encodePathSegment(id)}/revoke-sessions`, { method: 'POST' }); },
  register(payload: Credentials) {
    return request<User>('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  fetchRegistrationStatus() { return request<RegistrationStatus>('/api/auth/registration-status'); },
  login(payload: Credentials) {
    return request<User>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  me() {
    return request<AuthMeResponse>('/api/auth/me');
  },
  logout() {
    return request<void>('/api/auth/logout', { method: 'POST' });
  },
  changePassword(payload: { current_password: string; new_password: string }) {
    return request<void>('/api/auth/change-password', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  fetchSavedSearches() {
    return request<SavedSearch[]>('/api/me/saved-searches');
  },
  createSavedSearch(payload: SavedSearchPayload) {
    return request<SavedSearch>('/api/me/saved-searches', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  updateSavedSearch(id: string, payload: SavedSearchPayload) {
    return request<SavedSearch>(`/api/me/saved-searches/${encodePathSegment(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(payload)
    });
  },
  deleteSavedSearch(id: string) {
    return request<void>(`/api/me/saved-searches/${encodePathSegment(id)}`, { method: 'DELETE' });
  },
  markSavedSearchUsed(id: string) {
    return request<void>(`/api/me/saved-searches/${encodePathSegment(id)}/use`, { method: 'POST' });
  },
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
  searchLogs(bundleId: string, query: string, options?: { timeline?: string; path_like?: string; file_id?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.timeline) params.set('timeline', options.timeline);
    if (options?.path_like) params.set('path_like', options.path_like);
    if (options?.file_id) params.set('file_id', options.file_id);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<LogSearchResponse>(`/api/log/v2/${encodePathSegment(bundleId)}/search?${params.toString()}`);
  },
  searchIssueLogs(issueCode: string, query: string, options?: { mode?: 'filename' | 'content'; path_like?: string; from?: number; size?: number }) {
    const params = new URLSearchParams({ q: query });
    if (options?.mode) params.set('mode', options.mode);
    if (options?.path_like) params.set('path_like', options.path_like);
    if (typeof options?.from === 'number') params.set('from', String(options.from));
    if (typeof options?.size === 'number') params.set('size', String(options.size));
    return request<IssueLogSearchResponse>(`/api/issues/${encodePathSegment(normalizeIssueCode(issueCode))}/search?${params.toString()}`);
  },
  createTempResult(payload: { expression: string; bundle_hash?: string; file_id?: string; issue_code?: string; source_temp_id?: string }) {
    return request<TempResultInfo>('/api/temp-results', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  previewTempResult(payload: { expression: string; bundle_hash?: string; file_id?: string; issue_code?: string; source_temp_id?: string; from?: number; size?: number }) {
    return request<TempResultPreviewResponse>('/api/temp-results/preview', {
      method: 'POST',
      body: JSON.stringify(payload)
    });
  },
  fetchTempResult(id: string) {
    return request<TempResultInfo>(`/api/temp-results/${encodePathSegment(id)}`);
  },
  fetchTempResultLines(id: string, options?: { start?: number; limit?: number }) {
    const params = new URLSearchParams();
    if (typeof options?.start === 'number') params.set('start', String(options.start));
    if (typeof options?.limit === 'number') params.set('limit', String(options.limit));
    const query = params.toString();
    return request<TempResultLinesResponse>(`/api/temp-results/${encodePathSegment(id)}/lines${query ? `?${query}` : ''}`);
  },
  deleteTempResult(id: string) {
    return request<void>(`/api/temp-results/${encodePathSegment(id)}`, { method: 'DELETE' });
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
      xhr.withCredentials = true;
      xhr.timeout = 30 * 60 * 1000;
      xhr.setRequestHeader('Accept', 'application/json');
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          onProgress(Math.round((event.loaded / event.total) * 100));
        }
      };
      xhr.onload = () => {
        if (xhr.status < 200 || xhr.status >= 300) {
          if (shouldRevalidateAuthentication(xhr.status, xhr.responseText)) {
            window.dispatchEvent(new Event('rain:authentication-required'));
          }
          reject(parseErrorResponse(xhr.responseText, xhr.status));
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
