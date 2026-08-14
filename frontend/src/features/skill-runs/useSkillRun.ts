import { useCallback, useEffect, useState } from 'react';
import { ApiError, normalizeApiError, rainApi } from '../../api/client';
import type { SkillRun, SkillRunResult, SkillRunTimeScopeRequest } from '../../api/types';

const isActive = (run: SkillRun | null) => run?.status === 'QUEUED' || run?.status === 'RUNNING';

export function useSkillRun(issueCode: string) {
  const storageKey = `rain:skill-run:${issueCode}`;
  const [run, setRun] = useState<SkillRun | null>(null);
  const [result, setResult] = useState<SkillRunResult | null>(null);
  const [error, setError] = useState('');
  const refresh = useCallback(async (id: string) => {
    const value = await rainApi.fetchSkillRun(id);
    setRun(value);
    if (value.status === 'SUCCEEDED' && value.issue_code === issueCode) setResult(await rainApi.fetchSkillRunResult(id));
    if (!isActive(value) && value.issue_code !== issueCode) setRun(null);
    return value;
  }, [issueCode]);

  useEffect(() => {
    setRun(null); setResult(null); setError('');
    const id = sessionStorage.getItem(storageKey);
    if (id) {
      void refresh(id).catch(() => sessionStorage.removeItem(storageKey));
    } else {
      void rainApi.fetchActiveSkillRun().then((value) => { if (value) setRun(value); }).catch(() => undefined);
    }
  }, [refresh, storageKey]);

  useEffect(() => {
    if (!run || !isActive(run)) return;
    let stopped = false;
    const reload = () => void refresh(run.id).catch((reason) => { if (!stopped) setError(normalizeApiError(reason)); });
    const timer = window.setInterval(reload, 2000);
    let events: EventSource | null = null;
    if (typeof EventSource !== 'undefined') {
      events = new EventSource(rainApi.skillRunEventsUrl(run.id), { withCredentials: true });
      ['snapshot', 'run.started', 'tool.started', 'tool.completed', 'tool.rejected', 'tool.failed', 'iteration.completed', 'run.completed', 'run.failed', 'run.cancelled'].forEach((name) => events?.addEventListener(name, reload));
      events.onerror = reload;
    }
    return () => { stopped = true; window.clearInterval(timer); events?.close(); };
  }, [refresh, run?.id, run?.status]);

  const start = async (skillId: string, timeScope?: SkillRunTimeScopeRequest) => {
    setError(''); setResult(null);
    try {
      const value = await rainApi.createSkillRun(issueCode, skillId, timeScope);
      sessionStorage.setItem(storageKey, value.id);
      setRun(value);
      if (value.status === 'SUCCEEDED') setResult(await rainApi.fetchSkillRunResult(value.id));
    } catch (reason) {
      if (reason instanceof ApiError && reason.code === 'SKILL_RUN_ALREADY_ACTIVE') {
        const active = await rainApi.fetchActiveSkillRun().catch(() => null);
        if (active) setRun(active);
      }
      setError(normalizeApiError(reason));
    }
  };
  const cancel = async () => {
    if (!run) return;
    try { setRun(await rainApi.cancelSkillRun(run.id)); }
    catch (reason) { setError(normalizeApiError(reason)); }
  };
  return { run, result, error, active: isActive(run), start, cancel };
}
