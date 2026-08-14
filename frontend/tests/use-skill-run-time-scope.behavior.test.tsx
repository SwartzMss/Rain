import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useSkillRun } from '../src/features/skill-runs/useSkillRun';

const { createSkillRun, fetchActiveSkillRun } = vi.hoisted(() => ({
  createSkillRun: vi.fn(),
  fetchActiveSkillRun: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {},
  normalizeApiError: (value: unknown) => String(value),
  rainApi: {
    fetchSkillRun: vi.fn(),
    fetchSkillRunResult: vi.fn(),
    fetchActiveSkillRun,
    createSkillRun,
    cancelSkillRun: vi.fn(),
    skillRunEventsUrl: vi.fn(() => '/events')
  }
}));

describe('useSkillRun time scope forwarding', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    fetchActiveSkillRun.mockResolvedValue(null);
    createSkillRun.mockResolvedValue({
      id: 'run-1',
      issue_code: 'ISSUE-1',
      status: 'QUEUED'
    });
  });

  it('forwards a scope while preserving calls without a scope', async () => {
    const { result, unmount } = renderHook(() => useSkillRun('ISSUE-1'));
    const scope = { start: '2026-08-14 09:27:15.000', end: '2026-08-14 09:37:15.000' };

    await act(async () => { await result.current.start('skill-1', scope); });
    await act(async () => { await result.current.start('skill-2'); });

    expect(createSkillRun).toHaveBeenNthCalledWith(1, 'ISSUE-1', 'skill-1', {
      start: '2026-08-14 09:27:15.000',
      end: '2026-08-14 09:37:15.000'
    });
    expect(createSkillRun).toHaveBeenNthCalledWith(2, 'ISSUE-1', 'skill-2', undefined);
    unmount();
  });
});
