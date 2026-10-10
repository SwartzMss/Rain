import { useCallback, useEffect, useRef, useState } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { SavedSearch, SavedSearchPayload } from '../../../api/types';
import { PENDING_SAVED_SEARCH_KEY, takePendingSavedSearch } from '../pendingSavedSearch';
import type { ViewerTab } from '../viewerTabs';
import { finalizeSearchTokens, serializeSearchTokens, type SearchToken } from '../searchTokens';
import { detailEditorState } from './savedSearchEditor';
import type { SearchController } from './useSearchController';
import { LINE_PAGE_SIZE_OPTIONS } from '../linePageSizes';

type OpenViewerTab = (tab: ViewerTab) => void;

export function useSavedSearchController({
  authenticated,
  issueCode,
  locationPath,
  pendingSavedSearch,
  issueSearch,
  navigate,
  openViewerTab,
  waitForWorkspaceSession
}: {
  authenticated: boolean;
  issueCode: string;
  locationPath: string;
  pendingSavedSearch: SavedSearchPayload | null;
  issueSearch: SearchController;
  navigate: (path: string, options?: { state?: unknown }) => void;
  openViewerTab: OpenViewerTab;
  waitForWorkspaceSession: () => Promise<string | null>;
}) {
  const [savedSearches, setSavedSearches] = useState<SavedSearch[]>([]);
  const [savedSearchesOpen, setSavedSearchesOpen] = useState(false);
  const [saveDialogOpen, setSaveDialogOpen] = useState(Boolean(pendingSavedSearch));
  const [savedSearchName, setSavedSearchName] = useState('');
  const [error, setError] = useState('');
  const [editingSavedSearch, setEditingSavedSearch] = useState<SavedSearch | null>(null);
  const [editingSearchTokens, setEditingSearchTokens] = useState<SearchToken[]>([]);
  const [editingSearchDraft, setEditingSearchDraft] = useState('');
  const saveGenerationRef = useRef(0);
  const editingGenerationRef = useRef(0);

  const currentPayload = useCallback((): SavedSearchPayload | null => {
    try {
      const finalized = finalizeSearchTokens(issueSearch.tokens, issueSearch.draft);
      return {
        name: savedSearchName,
        search_type: 'DETAIL',
        query_text: serializeSearchTokens(finalized),
        options: { version: 1 }
      };
    } catch {
      return null;
    }
  }, [issueSearch.draft, issueSearch.tokens, savedSearchName]);

  const loadSavedSearches = useCallback(async () => {
    if (!authenticated) return;
    const items = await rainApi.fetchSavedSearches();
    setSavedSearches(items.filter((item) => item.search_type === 'DETAIL'));
  }, [authenticated]);

  useEffect(() => {
    if (!authenticated) {
      setSavedSearches([]);
      return;
    }
    void loadSavedSearches().catch((requestError) => setError(normalizeApiError(requestError)));
    if (!pendingSavedSearch) {
      const pending = takePendingSavedSearch(sessionStorage, true);
      if (pending) {
        const editor = detailEditorState(pending.query_text, pending.options);
        issueSearch.setEditor(editor.tokens);
        if (editor.error) issueSearch.setError(editor.error);
        setSaveDialogOpen(true);
      }
    }
  }, [authenticated, issueCode, issueSearch.setEditor, issueSearch.setError, loadSavedSearches, pendingSavedSearch]);

  const beginSaveSearch = useCallback(() => {
    const payload = currentPayload();
    if (!payload) {
      setError('请先输入有效搜索条件');
      return;
    }
    if (!authenticated) {
      sessionStorage.setItem(PENDING_SAVED_SEARCH_KEY, JSON.stringify(payload));
      navigate('/login', { state: { from: locationPath } });
      return;
    }
    saveGenerationRef.current += 1;
    setError('');
    setSaveDialogOpen(true);
  }, [authenticated, currentPayload, locationPath, navigate]);

  const saveSearch = useCallback(async () => {
    const generation = saveGenerationRef.current;
    const payload = currentPayload();
    if (!payload || !savedSearchName.trim()) {
      setError('请输入名称并确认搜索条件有效');
      return;
    }
    try {
      await rainApi.validateSearchExpression(payload.query_text);
      if (generation !== saveGenerationRef.current) return;
      await rainApi.createSavedSearch({ ...payload, name: savedSearchName.trim() });
      if (generation !== saveGenerationRef.current) return;
      setSavedSearchName('');
      setSaveDialogOpen(false);
      await loadSavedSearches();
    } catch (requestError) {
      setError(normalizeApiError(requestError));
    }
  }, [currentPayload, loadSavedSearches, savedSearchName]);

  const useSavedSearch = useCallback(async (item: SavedSearch) => {
    const editor = detailEditorState(item.query_text, item.options);
    if (editor.error) {
      issueSearch.setError(editor.error);
      return;
    }
    issueSearch.setEditor(editor.tokens);
    const response = await issueSearch.run({
      expression: item.query_text,
      payload: { expression: item.query_text, issue_code: issueCode, from: 0, size: LINE_PAGE_SIZE_OPTIONS[0] },
      scopeKey: `issue:${issueCode}`,
      workspaceSessionId: waitForWorkspaceSession(),
      onSuccess: (result) => {
        const hits = result.lines.map((line) => ({
          bundle_hash: line.bundle_hash,
          file_id: line.file_id ?? '',
          path: line.path,
          snippet: line.content,
          line_number: line.line_number
        }));
        openViewerTab({
          id: `search:${result.result_id}`,
          kind: 'search',
          resultId: result.result_id,
          title: item.name,
          pinned: false,
          scrollTop: 0,
          expression: item.query_text,
          hits,
          total: result.total,
          from: 0,
          pageSize: LINE_PAGE_SIZE_OPTIONS[0],
          pageHistory: [],
          source: { kind: 'issue', issueCode },
          queryPlan: {
            root: { kind: 'issue', issueCode },
            expressions: [item.query_text]
          }
        });
      }
    });
    if (!response) return;
    await rainApi.markSavedSearchUsed(item.id);
    setSavedSearchesOpen(false);
  }, [issueCode, issueSearch, openViewerTab, waitForWorkspaceSession]);

  const beginEditingSavedSearch = useCallback((item: SavedSearch) => {
    const editor = detailEditorState(item.query_text, item.options);
    setEditingSearchTokens(editor.tokens);
    setEditingSearchDraft('');
    setError(editor.error ?? '');
    editingGenerationRef.current += 1;
    setEditingSavedSearch({ ...item });
  }, []);

  const updateEditingSavedSearch = useCallback(async () => {
    if (!editingSavedSearch) return;
    const generation = editingGenerationRef.current;
    try {
      const tokens = finalizeSearchTokens(editingSearchTokens, editingSearchDraft);
      const queryText = serializeSearchTokens(tokens);
      await rainApi.validateSearchExpression(queryText);
      if (generation !== editingGenerationRef.current) return;
      await rainApi.updateSavedSearch(editingSavedSearch.id, {
        name: editingSavedSearch.name.trim(),
        search_type: 'DETAIL',
        query_text: queryText,
        options: { version: 1 },
        is_pinned: editingSavedSearch.is_pinned
      });
      if (generation !== editingGenerationRef.current) return;
      setEditingSavedSearch(null);
      await loadSavedSearches();
    } catch (requestError) {
      setError(normalizeApiError(requestError));
    }
  }, [editingSavedSearch, editingSearchDraft, editingSearchTokens, loadSavedSearches]);

  return {
    savedSearches,
    savedSearchesOpen,
    setSavedSearchesOpen,
    saveDialogOpen,
    setSaveDialogOpen,
    savedSearchName,
    setSavedSearchName,
    error,
    setError,
    editingSavedSearch,
    setEditingSavedSearch,
    editingSearchTokens,
    setEditingSearchTokens,
    editingSearchDraft,
    setEditingSearchDraft,
    beginSaveSearch,
    saveSearch,
    useSavedSearch,
    beginEditingSavedSearch,
    updateEditingSavedSearch,
    loadSavedSearches,
    bumpSaveGeneration: () => { saveGenerationRef.current += 1; },
    bumpEditingGeneration: () => { editingGenerationRef.current += 1; }
  };
}
