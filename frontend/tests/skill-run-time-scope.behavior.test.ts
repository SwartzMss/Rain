import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { toSkillRunTimeScope } from '../src/features/skill-runs/timeScope';

describe('skill run time scope request adapter', () => {
  it('returns no scope for the no-limit mode', () => {
    expect(toSkillRunTimeScope({ mode: 'none' })).toEqual({ scope: null, error: null });
  });

  it('passes an incident time and minute margins through without date arithmetic', () => {
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2026-08-14T09:30',
      beforeMinutes: '5',
      afterMinutes: '10'
    })).toEqual({
      scope: {
        incident_time: '2026-08-14T09:30',
        before_minutes: 5,
        after_minutes: 10
      },
      error: null
    });
  });

  it('passes a direct range through without normalizing or comparing dates', () => {
    expect(toSkillRunTimeScope({
      mode: 'range',
      start: '2026-08-14T10:00',
      end: '2026-08-14T09:00'
    })).toEqual({
      scope: {
        start: '2026-08-14T10:00',
        end: '2026-08-14T09:00'
      },
      error: null
    });
  });

  it('leaves date validity, ordering, and 24-hour limits to the backend', () => {
    expect(toSkillRunTimeScope({
      mode: 'range',
      start: 'not-a-date',
      end: 'also-not-a-date'
    }).error).toBeNull();
    expect(toSkillRunTimeScope({
      mode: 'incident',
      incidentTime: '2026-08-14T09:30',
      beforeMinutes: 1440,
      afterMinutes: 1
    }).error).toBeNull();
  });

  it('rejects missing inputs and invalid minute margins only', () => {
    expect(toSkillRunTimeScope({ mode: 'range', start: '', end: '2026-08-14T10:00' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '', beforeMinutes: '1', afterMinutes: '1' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '-1', afterMinutes: '1' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '1.5', afterMinutes: '1' }).error).toBeTruthy();
    expect(toSkillRunTimeScope({ mode: 'incident', incidentTime: '2026-08-14T09:30', beforeMinutes: '1', afterMinutes: '' }).error).toBeTruthy();
  });
});

describe('skill run API time scope payload', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ id: 'run-1' }), {
      status: 202,
      headers: { 'Content-Type': 'application/json' }
    }))));
  });

  it('sends incident, range, and explicit null requests without rewriting them', async () => {
    const incident = { incident_time: '2026-08-14T09:30', before_minutes: 5, after_minutes: 10 };
    const range = { start: '2026-08-14T10:00', end: '2026-08-14T09:00' };

    await rainApi.createSkillRun('issue-1', 'skill-1', incident);
    await rainApi.createSkillRun('issue-1', 'skill-1', range);
    await rainApi.createSkillRun('issue-1', 'skill-1');

    const calls = vi.mocked(fetch).mock.calls;
    expect(JSON.parse(String(calls[0]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: incident });
    expect(JSON.parse(String(calls[1]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: range });
    expect(JSON.parse(String(calls[2]?.[1]?.body))).toEqual({ skill_id: 'skill-1', time_scope: null });
  });
});
