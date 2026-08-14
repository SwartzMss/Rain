import type { SkillRunTimeScope } from '../../api/types';

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
  scope: SkillRunTimeScope | null;
  error: string | null;
}

const MAX_SCOPE_MILLIS = 24 * 60 * 60 * 1000;

function parseDateTime(value: string | undefined): Date | null {
  if (!value?.trim()) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function resultForRange(startMs: number, endMs: number): SkillRunTimeScopeResult {
  if (endMs <= startMs) {
    return { scope: null, error: '结束时间必须晚于开始时间' };
  }
  if (endMs - startMs > MAX_SCOPE_MILLIS) {
    return { scope: null, error: '分析时间范围不能超过 24 小时' };
  }
  return {
    scope: {
      start: new Date(startMs).toISOString(),
      end: new Date(endMs).toISOString()
    },
    error: null
  };
}

function parseMinutes(value: string | number | undefined): number | null {
  if (value === undefined || (typeof value === 'string' && !value.trim())) return null;
  const minutes = typeof value === 'number' ? value : Number(value);
  return Number.isInteger(minutes) && minutes >= 0 ? minutes : null;
}

export function toSkillRunTimeScope(input: SkillRunTimeScopeInput): SkillRunTimeScopeResult {
  if (input.mode === 'none') return { scope: null, error: null };

  if (input.mode === 'range') {
    const start = parseDateTime(input.start);
    const end = parseDateTime(input.end);
    if (!start || !end) return { scope: null, error: '请输入完整且有效的开始和结束时间' };
    return resultForRange(start.getTime(), end.getTime());
  }

  const incidentTime = parseDateTime(input.incidentTime);
  const beforeMinutes = parseMinutes(input.beforeMinutes);
  const afterMinutes = parseMinutes(input.afterMinutes);
  if (!incidentTime) return { scope: null, error: '请输入有效的故障时间' };
  if (beforeMinutes === null || afterMinutes === null) {
    return { scope: null, error: '故障前后分钟数必须是非负整数' };
  }

  const startMs = incidentTime.getTime() - beforeMinutes * 60 * 1000;
  const endMs = incidentTime.getTime() + afterMinutes * 60 * 1000;
  return resultForRange(startMs, endMs);
}
