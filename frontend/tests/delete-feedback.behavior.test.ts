import { describe, expect, it } from 'vitest';
import { ApiError } from '../src/api/client';
import { normalizeDeletionError } from '../src/features/files/deleteFeedback';

describe('delete conflict feedback', () => {
  it('explains a processing conflict instead of exposing a generic 409', () => {
    expect(normalizeDeletionError(new ApiError('conflict', 409, 'BUNDLE_PROCESSING'))).toBe(
      '当前有上传或处理中的 Bundle，暂不可删除，请等待处理完成后重试'
    );
  });

  it('keeps non-conflict errors actionable', () => {
    expect(normalizeDeletionError(new Error('network failure'))).toBe('network failure');
  });
});
