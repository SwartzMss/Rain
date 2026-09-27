import { useCallback, useEffect, useRef, useState } from 'react';
import { normalizeApiError, normalizeIssueCode, rainApi } from '../../../api/client';
import { useAuth } from '../../../auth/AuthContext';
import type { IssueSummary } from '../../../api/types';
import { isUser } from '../../../auth/permissions';

const LAST_ISSUE_STORAGE_KEY_PREFIX = 'rain:last_issue_id:';
const ISSUE_SCOPE_STORAGE_KEY_PREFIX = 'rain:issue_scope:';
const ISSUE_PAGE_SIZE = 50;

export type IssueListScope = 'mine' | 'all';

export function useIssues() {
  const auth = useAuth();
  const [selectedIssueCode, setSelectedIssueCode] = useState('');
  const [issueSearchText, setIssueSearchText] = useState('');
  const [issueScope, setIssueScope] = useState<IssueListScope>('all');
  const [issueError, setIssueError] = useState<string | null>(null);
  const [issues, setIssues] = useState<IssueSummary[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(false);
  const [issuesLoadingMore, setIssuesLoadingMore] = useState(false);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const skipPersistRef = useRef(false);
  const nextCursorRef = useRef<string | null>(null);
  const issueRequestRef = useRef(0);
  const storageKey =
    auth.state.status === 'AUTHENTICATED'
      ? `${LAST_ISSUE_STORAGE_KEY_PREFIX}${auth.state.user.id}`
      : null;
  const scopeStorageKey =
    auth.state.status === 'AUTHENTICATED' && isUser(auth.state.user)
      ? `${ISSUE_SCOPE_STORAGE_KEY_PREFIX}${auth.state.user.id}`
      : null;
  const canChooseScope = Boolean(scopeStorageKey);

  const currentIssueCode = selectedIssueCode.trim();

  const filteredIssues = issues;

  useEffect(() => {
    skipPersistRef.current = true;
    setSelectedIssueCode('');
    if (!storageKey) return;
    const stored = localStorage.getItem(storageKey);
    if (stored) {
      try {
        setSelectedIssueCode(normalizeIssueCode(stored));
      } catch {
        localStorage.removeItem(storageKey);
      }
    }
  }, [storageKey]);

  useEffect(() => {
    const stored = scopeStorageKey ? localStorage.getItem(scopeStorageKey) : null;
    const nextScope: IssueListScope = stored === 'all' || stored === 'mine'
      ? stored
      : canChooseScope
        ? 'mine'
        : 'all';
    setIssueScope(nextScope);
  }, [canChooseScope, scopeStorageKey]);

  useEffect(() => {
    if (skipPersistRef.current) {
      skipPersistRef.current = false;
      return;
    }
    if (currentIssueCode) {
      if (storageKey) localStorage.setItem(storageKey, currentIssueCode);
    } else if (storageKey) {
      localStorage.removeItem(storageKey);
    }
  }, [currentIssueCode, storageKey]);

  const loadIssues = useCallback(async (append = false) => {
    const cursor = append ? nextCursorRef.current : null;
    if (append && !cursor) return;
    const requestId = ++issueRequestRef.current;
    if (append) {
      setIssuesLoadingMore(true);
    } else {
      nextCursorRef.current = null;
      setNextCursor(null);
      setIssues([]);
      setIssuesLoading(true);
    }
    setIssuesError(null);
    try {
      const response = await rainApi.fetchIssues({
        scope: issueScope,
        query: issueSearchText,
        cursor: cursor ?? undefined,
        limit: ISSUE_PAGE_SIZE,
      });
      if (requestId !== issueRequestRef.current) return;
      nextCursorRef.current = response.next_cursor;
      setNextCursor(response.next_cursor);
      setIssues((current) => append
        ? [...current, ...response.items.filter((item) => !current.some((existing) => existing.code === item.code))]
        : response.items);
    } catch (error) {
      if (requestId !== issueRequestRef.current) return;
      setIssuesError(normalizeApiError(error));
    } finally {
      if (requestId !== issueRequestRef.current) return;
      if (append) setIssuesLoadingMore(false);
      else setIssuesLoading(false);
    }
  }, [issueScope, issueSearchText]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      loadIssues().catch(() => undefined);
    }, issueSearchText.trim() ? 300 : 0);
    return () => window.clearTimeout(timer);
  }, [loadIssues]);

  const loadMoreIssues = useCallback(() => loadIssues(true), [loadIssues]);

  const changeIssueScope = useCallback((scope: IssueListScope) => {
    if (!canChooseScope && scope !== 'all') return;
    setIssueScope(scope);
    if (scopeStorageKey) localStorage.setItem(scopeStorageKey, scope);
  }, [canChooseScope, scopeStorageKey]);

  const selectIssue = useCallback(
    (value: string) => {
      try {
        const code = normalizeIssueCode(value);
        setIssueError(null);
        setSelectedIssueCode(code);
        return code;
      } catch (error) {
        setIssueError(normalizeApiError(error));
        return null;
      }
    },
    []
  );

  const clearSelectedIssue = useCallback(() => {
    setSelectedIssueCode('');
  }, []);

  const createIssue = useCallback(async (rawCode: string) => {
    const code = normalizeIssueCode(rawCode);
    const issue = await rainApi.createIssue({ code });
    setIssues((prev) => [issue, ...prev.filter((item) => item.code !== issue.code)]);
    setSelectedIssueCode(issue.code);
    setIssueSearchText('');
    return issue;
  }, []);

  const deleteIssue = useCallback(
    async (code: string) => {
      await rainApi.deleteIssue(code);
      if (currentIssueCode === code) {
        setSelectedIssueCode('');
      }
      await loadIssues();
    },
    [currentIssueCode, loadIssues]
  );

  return {
    canChooseScope,
    changeIssueScope,
    clearSelectedIssue,
    createIssue,
    currentIssueCode,
    deleteIssue,
    filteredIssues,
    issueError,
    issueSearchText,
    issues,
    issuesError,
    issuesLoading,
    issuesLoadingMore,
    hasMoreIssues: Boolean(nextCursor),
    loadIssues,
    loadMoreIssues,
    selectIssue,
    selectedIssueCode,
    issueScope,
    setIssueSearchText,
    setIssuesError
  };
}
