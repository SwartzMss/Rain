import { useCallback, useEffect, useMemo, useState } from 'react';
import { normalizeApiError, normalizeIssueCode, rainApi } from '../../../api/client';
import type { IssueSummary } from '../../../api/types';

const LAST_ISSUE_STORAGE_KEY = 'rain:last_issue_id';

export function useIssues() {
  const [selectedIssueCode, setSelectedIssueCode] = useState('');
  const [issueSearchText, setIssueSearchText] = useState('');
  const [issueError, setIssueError] = useState<string | null>(null);
  const [issues, setIssues] = useState<IssueSummary[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(false);
  const [issuesError, setIssuesError] = useState<string | null>(null);

  const currentIssueCode = selectedIssueCode.trim();

  const filteredIssues = useMemo(() => {
    const filter = issueSearchText.trim().toLowerCase();
    if (!filter) return issues;
    return issues.filter(
      (issue) =>
        issue.code.toLowerCase().includes(filter) || issue.name.toLowerCase().includes(filter)
    );
  }, [issueSearchText, issues]);

  useEffect(() => {
    const stored = localStorage.getItem(LAST_ISSUE_STORAGE_KEY);
    if (stored) {
      try {
        setSelectedIssueCode(normalizeIssueCode(stored));
      } catch {
        localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
      }
    }
  }, []);

  useEffect(() => {
    if (currentIssueCode) {
      localStorage.setItem(LAST_ISSUE_STORAGE_KEY, currentIssueCode);
    } else {
      localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
    }
  }, [currentIssueCode]);

  const loadIssues = useCallback(async () => {
    setIssuesLoading(true);
    setIssuesError(null);
    try {
      setIssues(await rainApi.fetchIssues());
    } catch (error) {
      setIssuesError(normalizeApiError(error));
    } finally {
      setIssuesLoading(false);
    }
  }, []);

  useEffect(() => {
    loadIssues().catch(() => undefined);
  }, [loadIssues]);

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
    loadIssues,
    selectIssue,
    selectedIssueCode,
    setIssueSearchText,
    setIssuesError
  };
}
