import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { toSkillRunTimeScope } from '../src/features/skill-runs/timeScope';

describe('skill run time scope conversion', () => {
  it('returns no scope for the no-limit mode', () => {
    expect(toSkillRunTimeScope({ mode: 'none' })).toEqual({ scope: null, error: null });
  });

  it('converts an incident time and before/after minutes to local wall-clock strings', () => {
    const result = toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2026-08-14T09:30',
      beforeMinutes: '5',
      afterMinutes: '10'
    });

    expect(result.error).toBeNull();
    expect(result.scope).toEqual({
      start: '2026-08-14 09:25:00.000',
      end: '2026-08-14 09:40:00.000'
    });
    expect(result.scope?.start).not.toMatch(/[zZ]|[+-]\d\d:?\d\d$/);
  });

  it('preserves wall-clock fields when converting a direct range', () => {
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T09:00', end: '2026-08-14T10:00' })).toEqual({
      scope: {
        start: '2026-08-14 09:00:00.000',
        end: '2026-08-14 10:00:00.000'
      },
      error: null
    });
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14 09:00:15.125', end: '2026-08-14 09:30:15.5' })).toEqual({
      scope: {
        start: '2026-08-14 09:00:15.125',
        end: '2026-08-14 09:30:15.500'
      },
      error: null
    });
  });

  it('supports wall-clock arithmetic across a calendar boundary', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2026-08-14T00:05:15.250',
      beforeMinutes: '10',
      afterMinutes: '5'
    })).toEqual({
      scope: {
        start: '2026-08-13 23:55:15.250',
        end: '2026-08-14 00:10:15.250'
      },
      error: null
    });
  });

  it('moves backward across leap-year February', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2024-03-01T00:05',
      beforeMinutes: 10,
      afterMinutes: 10
    })).toEqual({
      scope: {
        start: '2024-02-29 23:55:00.000',
        end: '2024-03-01 00:15:00.000'
      },
      error: null
    });
  });

  it('moves backward across February in a non-leap year', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2023-03-01T00:05',
      beforeMinutes: 10,
      afterMinutes: 10
    })).toEqual({
      scope: {
        start: '2023-02-28 23:55:00.000',
        end: '2023-03-01 00:15:00.000'
      },
      error: null
    });
  });

  it('moves backward across a year boundary', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2024-01-01T00:05',
      beforeMinutes: 10,
      afterMinutes: 10
    })).toEqual({
      scope: {
        start: '2023-12-31 23:55:00.000',
        end: '2024-01-01 00:15:00.000'
      },
      error: null
    });
  });

  it('handles reverse month crossing when subtracting incident minutes', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2024-05-01T00:30',
      beforeMinutes: 90,
      afterMinutes: 30
    })).toEqual({
      scope: {
        start: '2024-04-30 23:00:00.000',
        end: '2024-05-01 01:00:00.000'
      },
      error: null
    });
  });

  it('rejects missing, equal, reversed, invalid, or overlong ranges', () => {
    expect(toSkillRunTimeScope({ mode: 'range', start: '', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T10:00', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T11:00', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-02-30T10:00', end: '2026-03-01T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T09:00', end: '2026-08-15T09:00:00.001' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T09:00+08:00', end: '2026-08-14T10:00+08:00' }).error).toBeTruthy();
  });

  it('rejects invalid incident margins and windows longer than 24 hours', () => {
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '-1', afterMinutes: '1' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '0', afterMinutes: '0' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '1440', afterMinutes: '1' }).error).toBeTruthy();
  });
});

describe('skill run API time scope payload', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ id: 'run-1' }), {
      status: 202,
      headers: { 'Content-Type': 'application/json' }
    }))));
  });

  it('sends the optional scope and keeps the explicit null payload for unscoped runs', async () => {
    const scope = { start: '2026-08-14 09:27:15.000', end: '2026-08-14 09:37:15.000' };

    await rainApi.createSkillRun('issue-1', 'skill-1', scope);
    await rainApi.createSkillRun('issue-1', 'skill-1');

    const calls = vi.mocked(fetch).mock.calls;
    expect(JSON.parse(String(calls[0]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: scope });
    expect(JSON.parse(String(calls[1]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: null });
  });
});
