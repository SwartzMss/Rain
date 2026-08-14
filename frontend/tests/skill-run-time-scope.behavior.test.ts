import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { toSkillRunTimeScope } from '../src/features/skill-runs/timeScope';

describe('skill run time scope conversion', () => {
  it('returns no scope for the no-limit mode', () => {
    expect(toSkillRunTimeScope({ mode: 'none' })).toEqual({ scope: null, error: null });
  });

  it('converts an incident time and before/after minutes to UTC RFC3339', () => {
    const result = toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2026-08-14T09:30',
      beforeMinutes: '5',
      afterMinutes: '10'
    });

    expect(result.error).toBeNull();
    expect(result.scope?.end).toBe(new Date('2026-08-14T09:40').toISOString());
    expect(result.scope?.start).toBe(new Date('2026-08-14T09:25').toISOString());
  });

  it('converts a direct range and rejects missing, equal, or reversed dates', () => {
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T09:00', end: '2026-08-14T10:00' })).toEqual({
      scope: {
        start: new Date('2026-08-14T09:00').toISOString(),
        end: new Date('2026-08-14T10:00').toISOString()
      },
      error: null
    });
    expect(toSkillRunTimeScope({ mode: 'range', start: '', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T10:00', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'range', start: '2026-08-14T11:00', end: '2026-08-14T10:00' }).error).toBeTruthy();
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
    const scope = { start: '2026-08-14T01:27:15.000Z', end: '2026-08-14T01:37:15.000Z' };

    await rainApi.createSkillRun('issue-1', 'skill-1', scope);
    await rainApi.createSkillRun('issue-1', 'skill-1');

    const calls = vi.mocked(fetch).mock.calls;
    expect(JSON.parse(String(calls[0]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: scope });
    expect(JSON.parse(String(calls[1]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: null });
  });
});
