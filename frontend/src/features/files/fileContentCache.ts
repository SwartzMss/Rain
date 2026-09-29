export const FILE_CONTENT_CACHE_MAX_BYTES = 64 * 1024 * 1024;
export const FILE_CONTENT_CACHE_MAX_PAGES = 20;
export const FILE_CONTENT_CACHE_MAX_CONCURRENT = 2;

export type FileContentRequestKey = Readonly<{
  bundle: string;
  file: string;
  context: string;
  open: string;
  start: number;
  requestedLimit: number;
}>;

export type FileContentLoader<T> = (
  request: FileContentRequestKey,
  signal: AbortSignal
) => Promise<T>;

export type FileContentCacheOptions<T> = {
  maxBytes?: number;
  maxPages?: number;
  maxConcurrent?: number;
  activeOversizedPageBudget?: number;
  estimateBytes?: (value: T, request: FileContentRequestKey) => number;
};

export type FileContentCacheStats = {
  bytes: number;
  pages: number;
  activeLoads: number;
  queuedLoads: number;
};

export type CurrentFileContent<T> = {
  request: FileContentRequestKey;
  value: T;
};

export type FileContentView<T> = {
  value: T | null;
  error: unknown | null;
};

const EMPTY_VIEW: FileContentView<unknown> = { value: null, error: null };

type CacheEntry<T> = {
  id: string;
  request: FileContentRequestKey;
  controller: AbortController;
  owners: Set<string>;
  status: 'queued' | 'loading' | 'resolved';
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
  value?: T;
  bytes: number;
  lastUsed: number;
  settled: boolean;
};

type OwnerState = {
  open: boolean;
  activeKey: string | null;
  current: CurrentFileContent<unknown> | undefined;
  viewKey: string | null;
  view: FileContentView<unknown>;
  listeners: Set<() => void>;
};

const requestId = (request: FileContentRequestKey): string => JSON.stringify([
  request.bundle,
  request.file,
  request.context,
  request.open,
  request.start,
  request.requestedLimit
]);

const defaultEstimateBytes = <T>(value: T): number => {
  if (value && typeof value === 'object' && 'byteLength' in value) {
    const byteLength = (value as { byteLength?: unknown }).byteLength;
    if (typeof byteLength === 'number' && Number.isFinite(byteLength)) return Math.max(0, byteLength);
  }

  try {
    return new TextEncoder().encode(JSON.stringify(value)).byteLength;
  } catch {
    return 0;
  }
};

function abortError(): Error {
  if (typeof DOMException !== 'undefined') return new DOMException('请求已取消', 'AbortError');
  const error = new Error('请求已取消');
  error.name = 'AbortError';
  return error;
}

function isAbortLike(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}

function sameFileInstance(
  left: FileContentRequestKey,
  right: FileContentRequestKey
): boolean {
  return left.bundle === right.bundle
    && left.file === right.file
    && left.context === right.context
    && left.open === right.open;
}

export class FileContentCache<T> {
  private readonly entries = new Map<string, CacheEntry<T>>();
  private readonly owners = new Map<string, OwnerState>();
  private readonly queue: CacheEntry<T>[] = [];
  private readonly maxBytes: number;
  private readonly maxPages: number;
  private readonly maxConcurrent: number;
  private readonly activeOversizedPageBudget: number;
  private readonly estimateBytes: (value: T, request: FileContentRequestKey) => number;
  private readonly errors = new Map<string, unknown>();
  private activeLoads = 0;
  private clock = 0;
  private residentBytes = 0;
  private activeOwner: string | null = null;

  constructor(
    private readonly loader: FileContentLoader<T>,
    options: FileContentCacheOptions<T> = {}
  ) {
    this.maxBytes = options.maxBytes ?? FILE_CONTENT_CACHE_MAX_BYTES;
    this.maxPages = options.maxPages ?? FILE_CONTENT_CACHE_MAX_PAGES;
    this.maxConcurrent = options.maxConcurrent ?? FILE_CONTENT_CACHE_MAX_CONCURRENT;
    this.activeOversizedPageBudget = options.activeOversizedPageBudget ?? 1;
    this.estimateBytes = options.estimateBytes ?? ((value) => defaultEstimateBytes(value));
  }

  request(owner: string, request: FileContentRequestKey): Promise<T> {
    const id = requestId(request);
    const ownerState = this.ownerState(owner);
    ownerState.open = true;
    this.activeOwner = owner;

    if (ownerState.activeKey !== id) {
      this.detachOwner(owner, ownerState.activeKey);
      ownerState.activeKey = id;
      ownerState.current = undefined;
      ownerState.viewKey = id;
      ownerState.view = EMPTY_VIEW;
      this.notify(ownerState);
    }

    let entry = this.entries.get(id);
    if (entry) {
      entry.owners.add(owner);
      entry.lastUsed = ++this.clock;
      if (entry.status === 'queued') {
        const index = this.queue.indexOf(entry);
        if (index >= 0) {
          this.queue.splice(index, 1);
          this.queue.unshift(entry);
        }
      }
      if (entry.status === 'resolved' && entry.value !== undefined) {
        ownerState.current = { request: entry.request, value: entry.value };
        ownerState.view = { value: entry.value, error: null };
        this.notify(ownerState);
      }
      return entry.promise;
    }

    let resolve!: (value: T) => void;
    let reject!: (error: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
      resolve = resolvePromise;
      reject = rejectPromise;
    });
    entry = {
      id,
      request: { ...request },
      controller: new AbortController(),
      owners: new Set([owner]),
      status: 'queued',
      promise,
      resolve,
      reject,
      bytes: 0,
      lastUsed: ++this.clock,
      settled: false
    };
    this.entries.set(id, entry);
    this.errors.delete(id);
    ownerState.viewKey = id;
    ownerState.view = EMPTY_VIEW;
    this.notify(ownerState);
    this.queue.push(entry);
    this.pump();
    return promise;
  }

  close(owner: string): void {
    const state = this.owners.get(owner);
    if (!state) return;
    state.open = false;
    this.detachOwner(owner, state.activeKey);
    state.activeKey = null;
    state.current = undefined;
    state.viewKey = null;
    state.view = EMPTY_VIEW;
    this.notify(state);
  }

  reopen(owner: string): void {
    const state = this.ownerState(owner);
    state.open = true;
    state.activeKey = null;
    state.current = undefined;
    state.viewKey = null;
    state.view = EMPTY_VIEW;
    this.notify(state);
  }

  reset(owner?: string): void {
    if (owner !== undefined) {
      const state = this.owners.get(owner);
      if (!state) return;
      const previousKey = state.activeKey;
      const previousRequest = previousKey ? this.entries.get(previousKey)?.request : undefined;
      this.detachOwner(owner, state.activeKey);
      state.activeKey = null;
      state.open = true;
      state.current = undefined;
      state.viewKey = null;
      state.view = EMPTY_VIEW;
      if (previousKey) {
        const entry = this.entries.get(previousKey);
        if (entry && entry.owners.size === 0) this.removeEntry(entry, false);
        this.errors.delete(previousKey);
      }
      if (previousRequest) {
        for (const entry of [...this.entries.values()]) {
          if (sameFileInstance(entry.request, previousRequest)) this.removeEntry(entry, true);
        }
      }
      this.notify(state);
      return;
    }

    for (const ownerId of this.owners.keys()) {
      const state = this.owners.get(ownerId);
      if (!state) continue;
      state.activeKey = null;
      state.current = undefined;
      state.viewKey = null;
      state.view = EMPTY_VIEW;
      this.notify(state);
    }
    for (const entry of [...this.entries.values()]) this.removeEntry(entry, true);
    this.errors.clear();
    this.queue.length = 0;
  }

  invalidate(request?: FileContentRequestKey): void {
    if (!request) {
      this.reset();
      return;
    }

    const id = requestId(request);
    const entry = this.entries.get(id);
    for (const [owner, state] of this.owners) {
      if (state.activeKey === id) {
        state.activeKey = null;
        state.current = undefined;
        state.viewKey = null;
        state.view = EMPTY_VIEW;
        this.notify(state);
      }
      entry?.owners.delete(owner);
    }
    if (entry) this.removeEntry(entry, true);
    this.errors.delete(id);
  }

  peek(request: FileContentRequestKey): T | undefined {
    const entry = this.entries.get(requestId(request));
    if (!entry || entry.status !== 'resolved') return undefined;
    entry.lastUsed = ++this.clock;
    return entry.value;
  }

  current(owner: string): CurrentFileContent<T> | undefined {
    const state = this.owners.get(owner);
    if (!state?.open) return undefined;
    return state.current as CurrentFileContent<T> | undefined;
  }

  getSnapshot(owner: string | null, request: FileContentRequestKey | null): FileContentView<T> {
    if (!owner || !request) return EMPTY_VIEW as FileContentView<T>;
    const state = this.owners.get(owner);
    if (!state || state.viewKey !== requestId(request)) return EMPTY_VIEW as FileContentView<T>;
    return state.view as FileContentView<T>;
  }

  subscribe(owner: string, listener: () => void): () => void {
    const state = this.ownerState(owner);
    state.listeners.add(listener);
    return () => state.listeners.delete(listener);
  }

  activate(owner: string): void {
    const state = this.owners.get(owner);
    if (!state?.activeKey) return;
    this.activeOwner = owner;
    const entry = this.entries.get(state.activeKey);
    if (entry) entry.lastUsed = ++this.clock;
  }

  stats(): FileContentCacheStats {
    return {
      bytes: this.residentBytes,
      pages: [...this.entries.values()].filter((entry) => entry.status === 'resolved').length,
      activeLoads: this.activeLoads,
      queuedLoads: this.queue.length
    };
  }

  private ownerState(owner: string): OwnerState {
    let state = this.owners.get(owner);
    if (!state) {
      state = {
        open: true,
        activeKey: null,
        current: undefined,
        viewKey: null,
        view: EMPTY_VIEW,
        listeners: new Set()
      };
      this.owners.set(owner, state);
    }
    return state;
  }

  private detachOwner(owner: string, id: string | null): void {
    if (!id) return;
    const entry = this.entries.get(id);
    if (!entry) return;
    entry.owners.delete(owner);
    if (entry.status === 'resolved' && entry.bytes > this.maxBytes && entry.owners.size === 0) {
      this.removeEntry(entry, false);
    } else if (entry.owners.size === 0 && entry.status !== 'resolved') {
      this.removeEntry(entry, true);
    }
    this.evictToBudget();
  }

  private pump(): void {
    while (this.activeLoads < this.maxConcurrent && this.queue.length > 0) {
      const entry = this.queue.shift();
      if (!entry) return;
      if (this.entries.get(entry.id) !== entry || entry.owners.size === 0) {
        this.rejectEntry(entry, abortError());
        continue;
      }

      entry.status = 'loading';
      this.activeLoads += 1;
      void this.loader(entry.request, entry.controller.signal).then(
        (value) => this.finishLoad(entry, value),
        (error) => this.failLoad(entry, error)
      );
    }
  }

  private finishLoad(entry: CacheEntry<T>, value: T): void {
    this.activeLoads -= 1;
    entry.settled = true;
    entry.value = value;
    entry.resolve(value);

    if (this.entries.get(entry.id) === entry) {
      const bytes = Math.max(0, this.estimateBytes(value, entry.request));
      const oversized = bytes > this.maxBytes;
      const activeOversized = [...this.entries.values()].filter(
        (candidate) => candidate !== entry
          && candidate.status === 'resolved'
          && candidate.bytes > this.maxBytes
          && this.isEntryActive(candidate)
      );

      if (!oversized || (this.isEntryActive(entry) && this.activeOversizedPageBudget > 0)) {
        for (const activePage of activeOversized) this.removeEntry(activePage, false);
        entry.status = 'resolved';
        entry.bytes = bytes;
        entry.lastUsed = ++this.clock;
        this.residentBytes += bytes;
        for (const candidate of [...this.entries.values()]) {
          if (candidate !== entry && candidate.status === 'resolved' && sameFileInstance(candidate.request, entry.request)) {
            this.removeEntry(candidate, false);
          }
        }
        for (const owner of entry.owners) {
          const state = this.owners.get(owner);
          if (!state || state.activeKey !== entry.id || !state.open) continue;
          state.current = { request: entry.request, value };
          state.viewKey = entry.id;
          state.view = { value, error: null };
          this.notify(state);
        }
        this.evictToBudget();
      } else {
        this.removeEntry(entry, false);
      }
    }

    this.pump();
  }

  private failLoad(entry: CacheEntry<T>, error: unknown): void {
    this.activeLoads -= 1;
    entry.settled = true;
    entry.reject(error);
    if (this.entries.get(entry.id) === entry) {
      if (!isAbortLike(error)) this.errors.set(entry.id, error);
      this.removeEntry(entry, false);
    }
    this.pump();
  }

  private rejectEntry(entry: CacheEntry<T>, error: unknown): void {
    if (entry.settled) return;
    entry.settled = true;
    entry.reject(error);
  }

  private removeEntry(entry: CacheEntry<T>, abort: boolean): void {
    if (this.entries.get(entry.id) === entry) {
      this.entries.delete(entry.id);
      if (entry.status === 'resolved') this.residentBytes -= entry.bytes;
    }
    if (abort) {
      if (entry.status === 'queued') {
        const index = this.queue.indexOf(entry);
        if (index >= 0) this.queue.splice(index, 1);
        this.rejectEntry(entry, abortError());
      } else if (entry.status === 'loading') {
        entry.controller.abort();
        this.rejectEntry(entry, abortError());
      }
    }
    for (const owner of this.owners) {
      const state = owner[1];
      if (state.activeKey === entry.id) {
        state.current = undefined;
        state.viewKey = entry.id;
        state.view = { value: null, error: this.errors.get(entry.id) ?? null };
        this.notify(state);
      }
    }
  }

  private notify(state: OwnerState): void {
    for (const listener of state.listeners) listener();
  }

  private isEntryActive(entry: CacheEntry<T>): boolean {
    if (!this.activeOwner) return false;
    const state = this.owners.get(this.activeOwner);
    return Boolean(state?.open && state.activeKey === entry.id);
  }

  private evictToBudget(): void {
    const resident = () => [...this.entries.values()].filter((entry) => entry.status === 'resolved');
    while (this.residentBytes > this.maxBytes || resident().length > this.maxPages) {
      const candidates = resident()
        .filter((entry) => !this.isEntryActive(entry))
        .sort((left, right) => left.lastUsed - right.lastUsed);
      const candidate = candidates[0];
      if (!candidate) break;
      this.removeEntry(candidate, false);
    }
  }
}

export function createFileContentCache<T>(
  loader: FileContentLoader<T>,
  options?: FileContentCacheOptions<T>
): FileContentCache<T> {
  return new FileContentCache(loader, options);
}
