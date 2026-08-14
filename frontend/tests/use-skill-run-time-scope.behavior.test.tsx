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

  it('forwards incident and range requests while preserving calls without a scope', async () => {
    const { result, unmount } = renderHook(() => useSkillRun('ISSUE-1'));
    const incident = { incident_time: '2026-08-14T09:30', before_minutes: 5, after_minutes: 10 };
    const range = { start: '2026-08-14T09:00', end: '2026-08-14T10:00' };

    await act(async () => { await result.current.start('skill-1', incident); });
    await act(async () => { await result.current.start('skill-2', range); });
    await act(async () => { await result.current.start('skill-3'); });

    expect(createSkillRun).toHaveBeenNthCalledWith(1, 'ISSUE-1', 'skill-1', incident);
    expect(createSkillRun).toHaveBeenNthCalledWith(2, 'ISSUE-1', 'skill-2', range);
    expect(createSkillRun).toHaveBeenNthCalledWith(3, 'ISSUE-1', 'skill-3', undefined);
    unmount();
  });
});
