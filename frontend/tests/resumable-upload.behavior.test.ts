import { beforeEach, describe, expect, it, vi } from 'vitest';
import { File as NodeFile } from 'node:buffer';
import { createHash, webcrypto } from 'node:crypto';
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

  it.each(['native', 'http', 'no-crypto', 'http-resume', 'http-to-native'])('uploads real chunks with correct SHA-256 using %s crypto', async (mode) => {
    vi.stubGlobal('crypto', mode === 'native' ? webcrypto : mode === 'no-crypto' ? undefined : {});
    const interrupted = mode === 'http-resume' || mode === 'http-to-native';
    let interruptNextChunk = interrupted;
    const session: UploadSessionResponse = {
      session_id: 'session-1',
      issue_code: 'ISSUE-1',
      file_name: 'large.log',
      file_size_bytes: RESUMABLE_UPLOAD_THRESHOLD_BYTES + 17,
      chunk_size_bytes: 8 * 1024 * 1024,
      committed_offset: 0,
      next_chunk_index: 0,
      status: 'OPEN',
      expires_at: '2099-01-01 00:00:00'
    };
    let committed = session;
    let completed = false;
    api.createUploadSession.mockResolvedValue(session);
    api.uploadUploadSessionChunk.mockImplementation(async (_id: string, index: number, offset: number, chunk: Blob, hash: string) => {
      const bytes = new Uint8Array(await chunk.arrayBuffer());
      expect(bytes.length).toBe(Math.min(session.chunk_size_bytes, session.file_size_bytes - offset));
      expect(hash).toBe(createHash('sha256').update(bytes).digest('hex'));
      if (index === 1 && interruptNextChunk) {
        interruptNextChunk = false;
        throw new Error('connection lost');
      }
      committed = {
        ...session,
        committed_offset: Math.min(session.file_size_bytes, offset + session.chunk_size_bytes),
        next_chunk_index: index + 1
      };
      return committed;
    });
    api.completeUploadSession.mockImplementation(async () => {
      completed = true;
      return { ...committed, status: 'FINALIZING' };
    });
    const internalBundleId = 'internal-bundle-1';
    const publicBundleHash = 'public-hash-1';
    api.fetchUploadSession.mockImplementation(async () => completed ? {
      ...committed,
      status: 'DELIVERED',
      bundle_id: internalBundleId,
      bundle_hash: publicBundleHash
    } : committed);
    api.fetchUploadTask.mockResolvedValue({
      task_id: publicBundleHash,
      issue_code: 'ISSUE-1',
      bundle_hash: publicBundleHash,
      status: 'PROCESSING',
      stage: 'RECEIVING',
      progress_percent: 0,
      total_bytes: session.file_size_bytes
    });

    const bytes = new Uint8Array(session.file_size_bytes);
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = i % 251;
    const file = new NodeFile([bytes], 'large.log', { lastModified: 1 }) as unknown as File;
    if (interrupted) {
      await expect(uploadFileWithResume('ISSUE-1', file, vi.fn())).rejects.toThrow('connection lost');
      if (mode === 'http-to-native') vi.stubGlobal('crypto', webcrypto);
    }
    const response = await uploadFileWithResume('ISSUE-1', file, vi.fn());

    expect(api.createUploadSession).toHaveBeenCalledTimes(1);
    expect(api.uploadUploadSessionChunk).toHaveBeenCalledTimes(interrupted ? 10 : 9);
    expect(api.uploadUploadSessionChunk.mock.calls.filter((call) => call[1] === 0)).toHaveLength(1);
    expect(api.completeUploadSession).toHaveBeenCalledWith('session-1');
    expect(api.fetchUploadTask).toHaveBeenCalledWith(publicBundleHash);
    expect(api.fetchUploadTask).not.toHaveBeenCalledWith(internalBundleId);
    expect(response.bundle_hash).toBe(publicBundleHash);
  });
});
