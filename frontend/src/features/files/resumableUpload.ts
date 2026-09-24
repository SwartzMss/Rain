import { ApiError, rainApi } from '../../api/client';
import type { UploadResponse, UploadSessionResponse } from '../../api/types';

export const RESUMABLE_UPLOAD_THRESHOLD_BYTES = 64 * 1024 * 1024;
const DATABASE_NAME = 'rain-upload-resume';
const DATABASE_VERSION = 1;
const STORE_NAME = 'sessions';
const MAX_FINALIZE_POLLS = 180;
const FINALIZE_POLL_DELAY_MS = 500;

interface UploadResumeRecord {
  key: string;
  sessionId: string;
  issueCode: string;
  fileName: string;
  fileSizeBytes: number;
  lastModifiedMs: number;
  chunkSizeBytes: number;
  idempotencyKey: string;
  chunkHashes: Record<string, string>;
}

const memoryRecords = new Map<string, UploadResumeRecord>();
let databasePromise: Promise<IDBDatabase | null> | undefined;

export function shouldUseResumableUpload(fileSizeBytes: number): boolean {
  return fileSizeBytes >= RESUMABLE_UPLOAD_THRESHOLD_BYTES;
}

function openDatabase(): Promise<IDBDatabase | null> {
  if (databasePromise) return databasePromise;
  if (typeof indexedDB === 'undefined') return Promise.resolve(null);
  databasePromise = new Promise((resolve) => {
    const request = indexedDB.open(DATABASE_NAME, DATABASE_VERSION);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE_NAME)) {
        request.result.createObjectStore(STORE_NAME, { keyPath: 'key' });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => resolve(null);
  });
  return databasePromise;
}

async function readRecord(key: string): Promise<UploadResumeRecord | null> {
  const database = await openDatabase();
  if (!database) return memoryRecords.get(key) ?? null;
  return new Promise((resolve) => {
    const request = database.transaction(STORE_NAME, 'readonly').objectStore(STORE_NAME).get(key);
    request.onsuccess = () => resolve((request.result as UploadResumeRecord | undefined) ?? null);
    request.onerror = () => resolve(memoryRecords.get(key) ?? null);
  });
}

async function writeRecord(record: UploadResumeRecord): Promise<void> {
  memoryRecords.set(record.key, record);
  const database = await openDatabase();
  if (!database) return;
  await new Promise<void>((resolve) => {
    const request = database.transaction(STORE_NAME, 'readwrite').objectStore(STORE_NAME).put(record);
    request.onsuccess = () => resolve();
    request.onerror = () => resolve();
  });
}

async function removeRecord(key: string): Promise<void> {
  memoryRecords.delete(key);
  const database = await openDatabase();
  if (!database) return;
  await new Promise<void>((resolve) => {
    const request = database.transaction(STORE_NAME, 'readwrite').objectStore(STORE_NAME).delete(key);
    request.onsuccess = () => resolve();
    request.onerror = () => resolve();
  });
}

async function sha256(value: ArrayBuffer): Promise<string> {
  if (!globalThis.crypto?.subtle) {
    throw new Error('当前浏览器不支持 Web Crypto，无法安全恢复上传');
  }
  const digest = await globalThis.crypto.subtle.digest('SHA-256', value);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

async function sha256Text(value: string): Promise<string> {
  return sha256(new TextEncoder().encode(value).buffer);
}

async function hashBlob(blob: Blob): Promise<string> {
  return sha256(await blob.arrayBuffer());
}

function identity(issueCode: string, file: File): string {
  return `${issueCode}\u0000${file.name}\u0000${file.size}\u0000${file.lastModified}`;
}

function nextIdempotencyKey(base: string, retry: boolean): string {
  return retry ? `${base.slice(0, 96)}-retry-${Date.now()}` : base;
}

function freshIdempotencyKey(): string {
  const random = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `rain-${random}`.slice(0, 128);
}

function sameFile(record: UploadResumeRecord, issueCode: string, file: File): boolean {
  return (
    record.issueCode === issueCode &&
    record.fileName === file.name &&
    record.fileSizeBytes === file.size &&
    record.lastModifiedMs === file.lastModified
  );
}

async function verifyLocalPrefix(
  record: UploadResumeRecord,
  session: UploadSessionResponse,
  file: File
): Promise<boolean> {
  if (session.committed_offset === 0) return true;
  if (session.committed_offset > file.size || session.chunk_size_bytes <= 0) return false;
  let offset = 0;
  let index = 0;
  while (offset < session.committed_offset) {
    const expectedSize = Math.min(session.chunk_size_bytes, file.size - offset);
    const expectedEnd = offset + expectedSize;
    if (expectedEnd > session.committed_offset) return false;
    const expectedHash = record.chunkHashes[String(index)];
    if (!expectedHash) return false;
    const actualHash = await hashBlob(file.slice(offset, expectedEnd));
    if (actualHash !== expectedHash) return false;
    offset = expectedEnd;
    index += 1;
  }
  return offset === session.committed_offset;
}

async function createFreshSession(
  issueCode: string,
  file: File,
  key: string,
  oldRecord?: UploadResumeRecord
): Promise<{ session: UploadSessionResponse; record: UploadResumeRecord }> {
  if (oldRecord) {
    await rainApi.deleteUploadSession(oldRecord.sessionId).catch(() => undefined);
  }
  const idempotencyKey = oldRecord
    ? nextIdempotencyKey(oldRecord.idempotencyKey, true)
    : freshIdempotencyKey();
  const session = await rainApi.createUploadSession(issueCode, {
    file_name: file.name,
    file_size_bytes: file.size,
    last_modified_ms: file.lastModified,
    idempotency_key: idempotencyKey
  });
  const record: UploadResumeRecord = {
    key,
    sessionId: session.session_id,
    issueCode,
    fileName: file.name,
    fileSizeBytes: file.size,
    lastModifiedMs: file.lastModified,
    chunkSizeBytes: session.chunk_size_bytes,
    idempotencyKey,
    chunkHashes: {}
  };
  await writeRecord(record);
  return { session, record };
}

async function findOrCreateSession(
  issueCode: string,
  file: File
): Promise<{ session: UploadSessionResponse; record: UploadResumeRecord }> {
  const key = `rain-${await sha256Text(identity(issueCode, file))}`;
  const stored = await readRecord(key);
  if (!stored || !sameFile(stored, issueCode, file)) {
    return createFreshSession(issueCode, file, key, stored ?? undefined);
  }

  let session: UploadSessionResponse;
  try {
    session = await rainApi.fetchUploadSession(stored.sessionId);
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 404) throw error;
    return createFreshSession(issueCode, file, key, stored);
  }

  if (
    session.file_name !== file.name ||
    session.file_size_bytes !== file.size ||
    session.status === 'CANCELLED' ||
    session.status === 'EXPIRED' ||
    session.status === 'FAILED'
  ) {
    return createFreshSession(issueCode, file, key, stored);
  }
  if (session.status === 'OPEN' && !(await verifyLocalPrefix(stored, session, file))) {
    return createFreshSession(issueCode, file, key, stored);
  }
  return { session, record: stored };
}

async function waitForDelivery(sessionId: string, session: UploadSessionResponse): Promise<UploadSessionResponse> {
  if (session.status === 'DELIVERED') return session;
  for (let attempt = 0; attempt < MAX_FINALIZE_POLLS; attempt += 1) {
    const current = await rainApi.fetchUploadSession(sessionId);
    if (current.status === 'DELIVERED') return current;
    if (current.status === 'FAILED' || current.status === 'CANCELLED' || current.status === 'EXPIRED') {
      throw new Error(current.failure_reason || '服务器未能完成上传处理');
    }
    await new Promise((resolve) => globalThis.setTimeout(resolve, FINALIZE_POLL_DELAY_MS));
  }
  throw new Error('上传已提交，但服务器处理时间过长，请稍后刷新文件列表');
}

async function uploadLargeFile(issueCode: string, file: File, onProgress: (percent: number) => void): Promise<UploadResponse> {
  let { session, record } = await findOrCreateSession(issueCode, file);
  if (session.status === 'DELIVERED') {
    const task = await rainApi.fetchUploadTask(session.bundle_id || '');
    await removeRecord(record.key);
    return {
      task_id: task.task_id,
      issue_code: task.issue_code,
      bundle_hash: task.bundle_hash,
      status: task.status,
      stage: task.stage,
      file_count: 1,
      total_bytes: task.total_bytes
    };
  }
  if (session.status === 'FINALIZING') {
    session = await waitForDelivery(session.session_id, session);
  } else {
    onProgress(Math.round((session.committed_offset / file.size) * 100));
    while (session.committed_offset < file.size) {
      const offset = session.committed_offset;
      const index = session.next_chunk_index;
      const chunk = file.slice(offset, Math.min(file.size, offset + session.chunk_size_bytes));
      const hash = await hashBlob(chunk);
      try {
        session = await rainApi.uploadUploadSessionChunk(session.session_id, index, offset, chunk, hash);
      } catch (error) {
        if (error instanceof ApiError && error.status === 409 && error.code === 'UPLOAD_OFFSET_CONFLICT') {
          session = await rainApi.fetchUploadSession(session.session_id);
          if (session.status !== 'OPEN' || !(await verifyLocalPrefix(record, session, file))) {
            throw error;
          }
          continue;
        }
        throw error;
      }
      record.chunkHashes[String(index)] = hash;
      await writeRecord(record);
      onProgress(Math.round((session.committed_offset / file.size) * 100));
    }
    session = await rainApi.completeUploadSession(session.session_id);
    session = await waitForDelivery(session.session_id, session);
  }

  if (!session.bundle_id) throw new Error('服务器未返回已交付的上传任务');
  const task = await rainApi.fetchUploadTask(session.bundle_id);
  await removeRecord(record.key);
  onProgress(100);
  return {
    task_id: task.task_id,
    issue_code: task.issue_code,
    bundle_hash: task.bundle_hash,
    status: task.status,
    stage: task.stage,
    file_count: 1,
    total_bytes: task.total_bytes
  };
}

export function uploadFileWithResume(
  issueCode: string,
  file: File,
  onProgress: (percent: number) => void
): Promise<UploadResponse> {
  if (!shouldUseResumableUpload(file.size)) {
    return rainApi.uploadLogs(issueCode, [file], onProgress);
  }
  return uploadLargeFile(issueCode, file, onProgress);
}
