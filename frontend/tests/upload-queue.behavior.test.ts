import { describe, expect, it } from 'vitest';
import { createUploadQueue } from '../src/features/files/uploadQueue';
import { createOptimisticUploadRows } from '../src/features/files/uploadRows';
import { stageLabel } from '../src/features/files/homeRows';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

async function settleQueue() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('upload queue', () => {
  it('runs at most two file uploads and starts the next file after acceptance', async () => {
    const requests = new Map<string, ReturnType<typeof deferred<{ bundle_hash: string }>>>();
    let active = 0;
    let peak = 0;
    const queue = createUploadQueue(async (_issueCode, file) => {
      active += 1;
      peak = Math.max(peak, active);
      const request = deferred<{ bundle_hash: string }>();
      requests.set(file.name, request);
      const result = await request.promise;
      active -= 1;
      return result;
    }, 2);

    queue.enqueue('ISSUE-1', [new File(['a'], 'a.log'), new File(['b'], 'b.log'), new File(['c'], 'c.log')]);
    await settleQueue();
    expect([...requests.keys()]).toEqual(['a.log', 'b.log']);
    expect(peak).toBe(2);

    requests.get('a.log')!.resolve({ bundle_hash: 'bundle-a' });
    await settleQueue();
    expect([...requests.keys()]).toEqual(['a.log', 'b.log', 'c.log']);
    expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.status).toBe('ACCEPTED');
  });

  it('marks one file failed without blocking the remaining queue', async () => {
    const requests = new Map<string, ReturnType<typeof deferred<{ bundle_hash: string }>>>();
    const queue = createUploadQueue(async (_issueCode, file) => {
      const request = deferred<{ bundle_hash: string }>();
      requests.set(file.name, request);
      return request.promise;
    }, 2);

    queue.enqueue('ISSUE-1', [new File(['a'], 'a.log'), new File(['b'], 'b.log'), new File(['c'], 'c.log')]);
    await settleQueue();
    requests.get('a.log')!.reject(Object.assign(new Error('bad upload'), { status: 400 }));
    await settleQueue();

    expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.status).toBe('FAILED');
    expect(requests.has('c.log')).toBe(true);
  });

  it('retries a 429 task after its server-provided delay and releases the slot while waiting', async () => {
    let attempts = 0;
    const queue = createUploadQueue(async () => {
      attempts += 1;
      if (attempts === 1) {
        throw Object.assign(new Error('busy'), { status: 429, retryAfterMs: 10 });
      }
      return { bundle_hash: 'bundle-a' };
    }, 1);

    queue.enqueue('ISSUE-1', [new File(['a'], 'a.log')]);
    await settleQueue();
    expect(queue.getTasks('ISSUE-1')[0].status).toBe('RETRY_WAIT');

    await new Promise<void>((resolve) => setTimeout(resolve, 15));
    await settleQueue();
    expect(attempts).toBe(2);
    expect(queue.getTasks('ISSUE-1')[0].status).toBe('ACCEPTED');
  });

  it('maps queue states to independent file rows with a retry target', () => {
    const rows = createOptimisticUploadRows([
      {
        id: 'task-failed',
        issueCode: 'ISSUE-1',
        file: new File(['a'], 'a.log'),
        name: 'a.log',
        sizeBytes: 1,
        status: 'UNCONFIRMED',
        progressPercent: 100,
        message: '接收结果未确认',
        response: null
      }
    ], new Set());

    expect(rows[0]).toMatchObject({
      key: 'task-failed',
      uploadTaskId: 'task-failed',
      stage: 'UNCONFIRMED',
      failureReason: '接收结果未确认'
    });
    expect(stageLabel('QUEUED')).toBe('等待上传');
    expect(stageLabel('RETRY_WAIT')).toBe('等待重试');
    expect(stageLabel('ACCEPTED')).toBe('已接收，等待处理');
  });
});
