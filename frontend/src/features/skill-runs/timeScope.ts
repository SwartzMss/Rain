import type { SkillRunTimeScopeRequest } from '../../api/types';

export type SkillRunTimeScopeMode = 'none' | 'incident' | 'range';

export interface SkillRunTimeScopeInput {
  mode: SkillRunTimeScopeMode;
  incidentTime?: string;
  beforeMinutes?: string | number;
  afterMinutes?: string | number;
  start?: string;
  end?: string;
}

export interface SkillRunTimeScopeResult {
  scope: SkillRunTimeScopeRequest | null;
  error: string | null;
}

function hasValue(value: string | undefined): value is string {
  return Boolean(value?.trim());
}

function parseMinutes(value: string | number | undefined): number | null {
  if (value === undefined || (typeof value === 'string' && !value.trim())) return null;
  const minutes = typeof value === 'number' ? value : Number(value);
  return Number.isSafeInteger(minutes) && minutes >= 0 ? minutes : null;
}

export function toSkillRunTimeScope(input: SkillRunTimeScopeInput): SkillRunTimeScopeResult {
  if (input.mode === 'none') return { scope: null, error: null };

  if (input.mode === 'range') {
    if (!hasValue(input.start) || !hasValue(input.end)) {
      return { scope: null, error: '请输入完整的开始和结束时间' };
    }
    return {
      scope: { start: input.start, end: input.end },
      error: null
    };
  }

  if (!hasValue(input.incidentTime)) return { scope: null, error: '请输入故障时间' };
  const beforeMinutes = parseMinutes(input.beforeMinutes);
  const afterMinutes = parseMinutes(input.afterMinutes);
  if (beforeMinutes === null || afterMinutes === null) {
    return { scope: null, error: '故障前后分钟数必须是非负整数' };
  }

  return {
    scope: {
      incident_time: input.incidentTime,
      before_minutes: beforeMinutes,
      after_minutes: afterMinutes
    },
    error: null
  };
}
