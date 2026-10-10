import { useCallback, useEffect, useRef, useState } from 'react';
import { rainApi } from '../../../api/client';

const RENEW_INTERVAL_MS = 5 * 60_000;
const REQUEST_TIMEOUT_MS = 15_000;
const BATCH_SIZE = 100;

// Opening a result is a renewable reference. Closing only stops renewal: other
// browser pages may still be using the same snapshot.
export function useResultRetention(resultIds: string[]) {
  const key = JSON.stringify([...new Set(resultIds)].sort());
  const [unavailableIds, setUnavailableIds] = useState<Set<string>>(new Set());
  const refreshRef = useRef<() => void>(() => {});
  const refresh = useCallback(() => refreshRef.current(), []);
  const markUnavailable = useCallback((id: string) => {
    setUnavailableIds((current) => current.has(id) ? current : new Set([...current, id]));
  }, []);

  useEffect(() => {
    const ids: string[] = JSON.parse(key);
    let disposed = false;
    let running = false;
    let pending = false;
    let controller: AbortController | null = null;
    let timeout: number | undefined;
    setUnavailableIds((current) => new Set([...current].filter((id) => ids.includes(id))));

    const renew = async () => {
      if (disposed || ids.length === 0) return;
      if (running) { pending = true; return; }
      running = true;
      try {
        for (let offset = 0; offset < ids.length && !disposed; offset += BATCH_SIZE) {
          const batch = ids.slice(offset, offset + BATCH_SIZE);
          controller = new AbortController();
          const requestController = controller;
          try {
            const response = await Promise.race([
              rainApi.keepAliveTempResults(batch, controller.signal),
              new Promise<never>((_, reject) => {
                timeout = window.setTimeout(() => {
                  requestController.abort();
                  reject(new Error('Result renewal timed out'));
                }, REQUEST_TIMEOUT_MS);
              })
            ]);
            if (disposed) return;
            setUnavailableIds((current) => {
              const next = new Set(current);
              for (const id of response.unavailable_ids) {
                if (batch.includes(id)) next.add(id);
              }
              return next;
            });
          } catch {
            // Offline/timeout is not evidence of expiry; the next cycle retries.
          } finally {
            window.clearTimeout(timeout);
          }
        }
      } finally {
        running = false;
        if (pending && !disposed) { pending = false; void renew(); }
      }
    };
    const trigger = () => { void renew(); };
    const visible = () => { if (document.visibilityState === 'visible') trigger(); };
    refreshRef.current = trigger;
    trigger();
    const interval = window.setInterval(trigger, RENEW_INTERVAL_MS);
    window.addEventListener('focus', trigger);
    window.addEventListener('online', trigger);
    document.addEventListener('visibilitychange', visible);
    return () => {
      disposed = true;
      controller?.abort();
      window.clearTimeout(timeout);
      window.clearInterval(interval);
      window.removeEventListener('focus', trigger);
      window.removeEventListener('online', trigger);
      document.removeEventListener('visibilitychange', visible);
      refreshRef.current = () => {};
    };
  }, [key]);

  return { unavailableIds, markUnavailable, refresh };
}
