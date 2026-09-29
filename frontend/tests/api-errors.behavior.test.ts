import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, normalizeApiError, rainApi } from '../src/api/client';
import { API_ERROR_CODES } from '../src/api/errorCodes';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('API error contract', () => {
  it('preserves a stable business code and public message from a 409 response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ code: 'ISSUE_ALREADY_EXISTS', message: '该 Issue 已存在' }), {
          status: 409,
          headers: { 'Content-Type': 'application/json' }
        })
      )
    );

    await expect(rainApi.createIssue({ code: 'DUPLICATE' })).rejects.toMatchObject({
      status: 409,
      code: 'ISSUE_ALREADY_EXISTS',
      message: '该 Issue 已存在'
    });
  });

  it('keeps generic conflicts and network errors safe', () => {
    expect(normalizeApiError(new ApiError('请求冲突', 409, 'CONFLICT'))).toBe('请求冲突');
    expect(normalizeApiError(new Error('networkerror'))).toBe('无法连接 Rain 后端，请确认服务已启动');
  });

  it('exposes only stable codes needed by current frontend branching', () => {
    expect(API_ERROR_CODES).toEqual({
      bundleProcessing: 'BUNDLE_PROCESSING',
      fileDeleteBundleBusy: 'FILE_DELETE_BUNDLE_BUSY',
      fileDeleteAlreadyRunning: 'FILE_DELETE_ALREADY_RUNNING'
    });
  });
});
