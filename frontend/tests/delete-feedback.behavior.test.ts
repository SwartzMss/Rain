import { describe, expect, it } from 'vitest';
import { ApiError } from '../src/api/client';
import { normalizeDeletionError } from '../src/features/files/deleteFeedback';

describe('delete conflict feedback', () => {
  it('explains a known busy-bundle deletion conflict', () => {
    expect(normalizeDeletionError(new ApiError('busy', 409, 'FILE_DELETE_BUNDLE_BUSY'))).toBe(
      '当前文件仍在处理中，暂不可删除，请等待处理完成后重试'
    );
  });

  it('explains a known competing-deletion conflict', () => {
    expect(normalizeDeletionError(new ApiError('running', 409, 'FILE_DELETE_ALREADY_RUNNING'))).toBe(
      '当前 Bundle 已有删除任务，请稍后重试'
    );
  });

  it('does not guess the meaning of an unknown 409', () => {
    expect(normalizeDeletionError(new ApiError('请求冲突', 409, 'CONFLICT'))).toBe('请求冲突');
  });

  it('keeps non-conflict errors actionable', () => {
    expect(normalizeDeletionError(new Error('network failure'))).toBe('network failure');
  });
});
