import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UploadSessionResponse } from '../src/api/types';

const api = vi.hoisted(() => ({
  uploadLogs: vi.fn(),
  createUploadSession: vi.fn(),
  fetchUploadSession: vi.fn(),
  deleteUploadSession: vi.fn(),
  uploadUploadSessionChunk: vi.fn(),
  completeUploadSession: vi.fn(),
  fetchUploadTask: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    status?: number;
    code?: string;
  },
  rainApi: api
}));

import {
  RESUMABLE_UPLOAD_THRESHOLD_BYTES,
  shouldUseResumableUpload,
  uploadFileWithResume
} from '../src/features/files/resumableUpload';

describe('resumable upload boundary', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('indexedDB', undefined);
    vi.stubGlobal('crypto', {
      subtle: {
        digest: vi.fn(async () => new ArrayBuffer(32))
      }
    });
    if (!Blob.prototype.arrayBuffer) {
      Object.defineProperty(Blob.prototype, 'arrayBuffer', {
        configurable: true,
        value: async function arrayBuffer(this: Blob) {
          return new ArrayBuffer(this.size);
        }
      });
    }
  });

  it('keeps files below 64 MiB on multipart and sessions at or above the threshold', () => {
    expect(shouldUseResumableUpload(RESUMABLE_UPLOAD_THRESHOLD_BYTES - 1)).toBe(false);
    expect(shouldUseResumableUpload(RESUMABLE_UPLOAD_THRESHOLD_BYTES)).toBe(true);
    expect(shouldUseResumableUpload(RESUMABLE_UPLOAD_THRESHOLD_BYTES + 1)).toBe(true);
  });

  it('resumes a large file through session chunks and finalization', async () => {
    const session: UploadSessionResponse = {
      session_id: 'session-1',
      issue_code: 'ISSUE-1',
      file_name: 'large.log',
      file_size_bytes: RESUMABLE_UPLOAD_THRESHOLD_BYTES,
      chunk_size_bytes: 8 * 1024 * 1024,
      committed_offset: 0,
      next_chunk_index: 0,
      status: 'OPEN',
      expires_at: '2099-01-01 00:00:00'
    };
    api.createUploadSession.mockResolvedValue(session);
    api.uploadUploadSessionChunk.mockImplementation(async (_id: string, index: number, offset: number) => ({
      ...session,
      committed_offset: Math.min(session.file_size_bytes, offset + session.chunk_size_bytes),
      next_chunk_index: index + 1
    }));
    api.completeUploadSession.mockResolvedValue({ ...session, status: 'FINALIZING' });
    api.fetchUploadSession.mockResolvedValue({ ...session, status: 'DELIVERED', bundle_id: 'bundle-1' });
    api.fetchUploadTask.mockResolvedValue({
      task_id: 'bundle-1',
      issue_code: 'ISSUE-1',
      bundle_hash: 'bundle-1',
      status: 'PROCESSING',
      stage: 'RECEIVING',
      progress_percent: 0,
      total_bytes: session.file_size_bytes
    });

    const file = new File(['x'], 'large.log');
    Object.defineProperty(file, 'size', { configurable: true, value: session.file_size_bytes });
    const response = await uploadFileWithResume('ISSUE-1', file, vi.fn());

    expect(api.createUploadSession).toHaveBeenCalledTimes(1);
    expect(api.uploadUploadSessionChunk).toHaveBeenCalledTimes(8);
    expect(api.completeUploadSession).toHaveBeenCalledWith('session-1');
    expect(response.bundle_hash).toBe('bundle-1');
  });
});
