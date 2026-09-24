export type UploadQueueTaskStatus =
  | 'QUEUED'
  | 'UPLOADING'
  | 'RETRY_WAIT'
  | 'ACCEPTED'
  | 'FAILED'
  | 'UNCONFIRMED';

export type UploadQueueTask<TResponse> = {
  id: string;
  issueCode: string;
  file: File;
  name: string;
  sizeBytes: number;
  status: UploadQueueTaskStatus;
  progressPercent: number;
  message: string | null;
  response: TResponse | null;
};

export type UploadFileOperation<TResponse> = (
  issueCode: string,
  file: File,
  onProgress: (percent: number) => void
) => Promise<TResponse>;

type RetryableError = {
  status: 429;
  retryAfterMs?: number;
};

let taskSequence = 0;

const taskId = () => `upload-task-${Date.now()}-${taskSequence++}`;

const errorStatus = (error: unknown) => {
  if (typeof error !== 'object' || error === null || !('status' in error)) return undefined;
  const status = Number(error.status);
  return Number.isFinite(status) ? status : undefined;
};

const isRateLimited = (error: unknown): error is RetryableError =>
  errorStatus(error) === 429;

const isUnconfirmed = (error: unknown) => {
  const status = errorStatus(error);
  return status === undefined || status >= 500;
};

const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : String(error || '上传失败');

const retryDelay = (error: RetryableError, attempt: number) => {
  if (typeof error.retryAfterMs === 'number' && Number.isFinite(error.retryAfterMs)) {
    return Math.max(0, Math.min(error.retryAfterMs, 30_000));
  }
  const base = Math.min(30_000, 500 * 2 ** attempt);
  return base + Math.round(Math.random() * Math.min(base, 250));
};

export function createUploadQueue<TResponse>(
  uploadFile: UploadFileOperation<TResponse>,
  concurrency = 2
) {
  if (!Number.isInteger(concurrency) || concurrency < 1) {
    throw new Error('上传并发数必须是正整数');
  }

  const tasks = new Map<string, UploadQueueTask<TResponse>>();
  const pending: string[] = [];
  const listeners = new Set<() => void>();
  let active = 0;
  let snapshot: readonly UploadQueueTask<TResponse>[] = [];

  const notify = () => {
    snapshot = [...tasks.values()];
    listeners.forEach((listener) => listener());
  };

  const update = (id: string, changes: Partial<UploadQueueTask<TResponse>>) => {
    const current = tasks.get(id);
    if (!current) return;
    tasks.set(id, { ...current, ...changes });
    notify();
  };

  const runTask = async (id: string, attempt: number): Promise<void> => {
    const current = tasks.get(id);
    if (!current) return;

    update(id, { status: 'UPLOADING', message: null });
    try {
      const response = await uploadFile(current.issueCode, current.file, (percent) => {
        update(id, {
          progressPercent: Math.max(0, Math.min(100, Math.round(percent)))
        });
      });
      update(id, {
        status: 'ACCEPTED',
        progressPercent: 100,
        message: null,
        response
      });
    } catch (error) {
      if (isRateLimited(error) && attempt < 2) {
        const delay = retryDelay(error, attempt);
        update(id, {
          status: 'RETRY_WAIT',
          message: `服务器繁忙，将在 ${Math.ceil(delay / 1000)} 秒后重试`
        });
        window.setTimeout(() => {
          const task = tasks.get(id);
          if (!task || task.status !== 'RETRY_WAIT') return;
          pending.push(id);
          update(id, { status: 'QUEUED', message: null });
          drain();
        }, delay);
      } else if (isUnconfirmed(error)) {
        update(id, {
          status: 'UNCONFIRMED',
          message: '接收结果未确认，请刷新文件列表核对；确认未接收后可重试'
        });
      } else {
        update(id, { status: 'FAILED', message: errorMessage(error) });
      }
    } finally {
      active -= 1;
      drain();
    }
  };

  const drain = () => {
    while (active < concurrency && pending.length > 0) {
      const id = pending.shift();
      if (!id) continue;
      const task = tasks.get(id);
      if (!task || task.status !== 'QUEUED') continue;
      active += 1;
      void runTask(id, 0);
    }
  };

  const enqueue = (issueCode: string, files: File[]) => {
    const ids = files.map((file) => {
      const id = taskId();
      tasks.set(id, {
        id,
        issueCode,
        file,
        name: file.name,
        sizeBytes: file.size,
        status: 'QUEUED',
        progressPercent: 0,
        message: null,
        response: null
      });
      pending.push(id);
      return id;
    });
    notify();
    drain();
    return ids;
  };

  const retry = (id: string) => {
    const task = tasks.get(id);
    if (!task || (task.status !== 'FAILED' && task.status !== 'UNCONFIRMED')) return;
    update(id, {
      status: 'QUEUED',
      progressPercent: 0,
      message: null,
      response: null
    });
    pending.push(id);
    drain();
  };

  return {
    enqueue,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getSnapshot() {
      return snapshot;
    },
    getTasks(issueCode?: string) {
      return issueCode ? snapshot.filter((task) => task.issueCode === issueCode) : [...snapshot];
    },
    retry
  };
}
