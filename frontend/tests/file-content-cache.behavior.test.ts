import { beforeEach, describe, expect, it } from 'vitest';
import {
  FILE_CONTENT_CACHE_MAX_BYTES,
  FileContentCache,
  type FileContentRequestKey
} from '../src/features/files/fileContentCache';

type Page = { id: string; bytes: number };

const key = (overrides: Partial<FileContentRequestKey> = {}): FileContentRequestKey => ({
  bundle: 'bundle-1',
  file: 'file-1',
  context: 'text',
  open: 'open-1',
  start: 0,
  requestedLimit: 100,
  ...overrides
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('file content cache', () => {
  beforeEach(() => {
    // Keep each test independent without relying on fake timers.
  });

  it('uses every request dimension in the cache key', async () => {
    const requests: FileContentRequestKey[] = [];
    const cache = new FileContentCache<Page>(async (request) => {
      requests.push(request);
      return { id: JSON.stringify(request), bytes: 1 };
    });

    const variants: FileContentRequestKey[] = [
      key(),
      key({ bundle: 'bundle-2' }),
      key({ file: 'file-2' }),
      key({ context: 'source-hit' }),
      key({ open: 'open-2' }),
      key({ start: 100 }),
      key({ requestedLimit: 200 })
    ];

    for (const [index, request] of variants.entries()) {
      await cache.request(`owner-${index}`, request);
    }

    expect(requests).toHaveLength(7);
    expect(new Set(requests.map((request) => JSON.stringify(request))).size).toBe(7);
  });

  it('deduplicates concurrent requests with the same complete key', async () => {
    const pending = deferred<Page>();
    let loads = 0;
    const cache = new FileContentCache<Page>(async () => {
      loads += 1;
      return pending.promise;
    });

    const first = cache.request('owner-a', key());
    const second = cache.request('owner-b', key());
    expect(loads).toBe(1);

    pending.resolve({ id: 'shared', bytes: 1 });
    await expect(Promise.all([first, second])).resolves.toEqual([
      { id: 'shared', bytes: 1 },
      { id: 'shared', bytes: 1 }
    ]);
  });

  it('does not publish a closed request after the owner reopens', async () => {
    const first = deferred<Page>();
    const second = deferred<Page>();
    let loads = 0;
    const cache = new FileContentCache<Page>(async (_request, signal) => {
      loads += 1;
      return loads === 1 ? first.promise : second.promise;
    });

    const oldRequest = cache.request('tab-1', key());
    cache.close('tab-1');
    cache.reopen('tab-1');
    const reopenedRequest = cache.request('tab-1', key());

    expect(loads).toBe(2);
    first.resolve({ id: 'stale', bytes: 1 });
    second.resolve({ id: 'fresh', bytes: 1 });
    await expect(oldRequest).rejects.toMatchObject({ name: 'AbortError' });
    await expect(reopenedRequest).resolves.toEqual({ id: 'fresh', bytes: 1 });
    expect(cache.peek(key())).toEqual({ id: 'fresh', bytes: 1 });
    expect(cache.current('tab-1')?.value).toEqual({ id: 'fresh', bytes: 1 });
  });

  it('reset clears the owner page and invalidate forces a fresh request', async () => {
    let loads = 0;
    const cache = new FileContentCache<Page>(async () => ({ id: `page-${++loads}`, bytes: 1 }));

    await cache.request('tab-1', key());
    cache.reset('tab-1');
    expect(cache.peek(key())).toBeUndefined();
    await cache.request('tab-1', key());
    expect(loads).toBe(2);

    cache.invalidate(key());
    expect(cache.peek(key())).toBeUndefined();
    await expect(cache.request('tab-1', key())).resolves.toEqual({ id: 'page-3', bytes: 1 });
    expect(loads).toBe(3);
  });

  it('keeps the newest owner request current when responses arrive out of order', async () => {
    const older = deferred<Page>();
    const newer = deferred<Page>();
    const cache = new FileContentCache<Page>(async (request) =>
      request.start === 0 ? older.promise : newer.promise
    );

    const oldRequest = cache.request('tab-1', key({ start: 0 }));
    const newRequest = cache.request('tab-1', key({ start: 100 }));

    newer.resolve({ id: 'new', bytes: 1 });
    await newRequest;
    older.resolve({ id: 'old', bytes: 1 });
    await expect(oldRequest).rejects.toMatchObject({ name: 'AbortError' });

    expect(cache.current('tab-1')).toMatchObject({
      request: key({ start: 100 }),
      value: { id: 'new', bytes: 1 }
    });
  });

  it('keeps only the newest successful page for one file instance', async () => {
    const cache = new FileContentCache<Page>(async (request) => ({ id: String(request.start), bytes: 1 }));

    await cache.request('tab-1', key({ start: 0 }));
    await cache.request('tab-1', key({ start: 100 }));

    expect(cache.peek(key({ start: 0 }))).toBeUndefined();
    expect(cache.peek(key({ start: 100 }))).toEqual({ id: '100', bytes: 1 });
  });

  it('aborts orphaned work and starts no more than two loaders at once', async () => {
    const pending = [deferred<Page>(), deferred<Page>(), deferred<Page>()];
    const signals: AbortSignal[] = [];
    let loads = 0;
    const cache = new FileContentCache<Page>(async (_request, signal) => {
      signals.push(signal);
      const index = loads++;
      signal.addEventListener('abort', () => pending[index].reject(new DOMException('aborted', 'AbortError')));
      return pending[index].promise;
    });

    const first = cache.request('owner-1', key({ file: 'file-1' }));
    const second = cache.request('owner-2', key({ file: 'file-2' }));
    const third = cache.request('owner-3', key({ file: 'file-3' }));
    expect(loads).toBe(2);

    cache.close('owner-2');
    expect(signals[1].aborted).toBe(true);
    pending[0].resolve({ id: 'first', bytes: 1 });
    await first;
    expect(loads).toBe(3);

    pending[2].resolve({ id: 'third', bytes: 1 });
    await third;
    pending[1].resolve({ id: 'aborted', bytes: 1 });
    await expect(second).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('enforces the 64 MiB and 20-page limits with LRU eviction', async () => {
    const cache = new FileContentCache<Page>(async (request) => ({
      id: request.file,
      bytes: request.file === 'large' ? 40 * 1024 * 1024 : 1
    }), { estimateBytes: (page) => page.bytes });

    await cache.request('owner-a', key({ file: 'a' }));
    cache.close('owner-a');
    await cache.request('owner-b', key({ file: 'b' }));
    cache.close('owner-b');
    expect(cache.peek(key({ file: 'a' }))).toBeDefined();
    cache.peek(key({ file: 'a' }));

    await cache.request('owner-large', key({ file: 'large' }));
    cache.close('owner-large');
    expect(cache.stats()).toMatchObject({ bytes: 40 * 1024 * 1024 + 2, pages: 3 });
    expect(cache.peek(key({ file: 'b' }))).toBeDefined();

    for (let index = 0; index < 21; index += 1) {
      const owner = `owner-${index}`;
      await cache.request(owner, key({ file: `page-${index}` }));
      cache.close(owner);
    }

    expect(cache.stats().pages).toBe(20);
    expect(cache.peek(key({ file: 'page-0' }))).toBeUndefined();
    expect(cache.peek(key({ file: 'page-1' }))).toBeDefined();
  });

  it('allows one active oversized page but does not retain a second one', async () => {
    const oversized = 80 * 1024 * 1024;
    let loads = 0;
    const cache = new FileContentCache<Page>(async (request) => ({
      id: `${request.file}-${++loads}`,
      bytes: oversized
    }), { estimateBytes: (page) => page.bytes });

    await cache.request('owner-a', key({ file: 'large-a' }));
    cache.activate('owner-a');
    expect(cache.peek(key({ file: 'large-a' }))).toEqual({ id: 'large-a-1', bytes: oversized });

    await cache.request('owner-b', key({ file: 'large-b' }));
    expect(cache.peek(key({ file: 'large-b' }))).toEqual({ id: 'large-b-2', bytes: oversized });
    expect(cache.peek(key({ file: 'large-a' }))).toBeUndefined();
    expect(cache.stats()).toMatchObject({ pages: 1, bytes: oversized });

    cache.close('owner-b');
    await cache.request('owner-a', key({ file: 'large-a' }));
    cache.activate('owner-a');
    expect(cache.peek(key({ file: 'large-a' }))).toEqual({ id: 'large-a-3', bytes: oversized });
    expect(cache.stats().pages).toBe(1);
    expect(FILE_CONTENT_CACHE_MAX_BYTES).toBe(64 * 1024 * 1024);
  });
});
