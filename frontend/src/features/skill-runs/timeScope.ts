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

const MINUTES_PER_DAY = 24 * 60;
const MILLIS_PER_MINUTE = 60 * 1000;
const MAX_SCOPE_MILLIS = MINUTES_PER_DAY * MILLIS_PER_MINUTE;

interface WallClockTime {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
  millisecond: number;
  // This is a sortable wall-clock comparison key, not a Unix timestamp.
  comparisonKey: number;
}

function isLeapYear(year: number): boolean {
  return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return isLeapYear(year) ? 29 : 28;
  return [4, 6, 9, 11].includes(month) ? 30 : 31;
}

function daysBeforeYear(year: number): number {
  const previousYear = year - 1;
  return previousYear * 365 + Math.floor(previousYear / 4) - Math.floor(previousYear / 100) + Math.floor(previousYear / 400);
}

function daysBeforeMonth(year: number, month: number): number {
  let days = 0;
  for (let currentMonth = 1; currentMonth < month; currentMonth += 1) {
    days += daysInMonth(year, currentMonth);
  }
  return days;
}

function comparisonKeyFor(parts: Omit<WallClockTime, 'comparisonKey'>): number {
  // The year-one calendar origin is only an internal encoding baseline. It
  // gives wall-clock values a stable ordering without assigning UTC meaning.
  const calendarDay = daysBeforeYear(parts.year) + daysBeforeMonth(parts.year, parts.month) + parts.day - 1;
  return (((calendarDay * 24 + parts.hour) * 60 + parts.minute) * 60 + parts.second) * 1000 + parts.millisecond;
}

function createWallClockTime(parts: Omit<WallClockTime, 'comparisonKey'>): WallClockTime | null {
  if (parts.year < 1 || parts.year > 9999) return null;
  if (parts.month < 1 || parts.month > 12) return null;
  if (parts.day < 1 || parts.day > daysInMonth(parts.year, parts.month)) return null;
  if (parts.hour < 0 || parts.hour > 23) return null;
  if (parts.minute < 0 || parts.minute > 59) return null;
  if (parts.second < 0 || parts.second > 59) return null;
  if (parts.millisecond < 0 || parts.millisecond > 999) return null;
  return { ...parts, comparisonKey: comparisonKeyFor(parts) };
}

function parseWallClock(value: string | undefined): WallClockTime | null {
  if (!value?.trim()) return null;

  const match = value.trim().match(/^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,3}))?)?$/);
  if (!match) return null;

  const fraction = match[7] ?? '';
  return createWallClockTime({
    year: Number(match[1]),
    month: Number(match[2]),
    day: Number(match[3]),
    hour: Number(match[4]),
    minute: Number(match[5]),
    second: Number(match[6] ?? 0),
    millisecond: fraction ? Number(fraction.padEnd(3, '0')) : 0
  });
}

function shiftCalendarDay(time: WallClockTime, dayDelta: number): { year: number; month: number; day: number } | null {
  let { year, month, day } = time;
  while (dayDelta > 0) {
    day += 1;
    if (day > daysInMonth(year, month)) {
      day = 1;
      month += 1;
      if (month > 12) {
        month = 1;
        year += 1;
      }
    }
    dayDelta -= 1;
  }
  while (dayDelta < 0) {
    day -= 1;
    if (day < 1) {
      month -= 1;
      if (month < 1) {
        month = 12;
        year -= 1;
      }
      day = daysInMonth(year, month);
    }
    dayDelta += 1;
  }
  return year >= 1 && year <= 9999 ? { year, month, day } : null;
}

function addMinutes(time: WallClockTime, minutes: number): WallClockTime | null {
  const totalMinutes = time.hour * 60 + time.minute + minutes;
  const dayDelta = Math.floor(totalMinutes / MINUTES_PER_DAY);
  const minuteOfDay = totalMinutes - dayDelta * MINUTES_PER_DAY;
  const date = shiftCalendarDay(time, dayDelta);
  if (!date) return null;

  return createWallClockTime({
    ...date,
    hour: Math.floor(minuteOfDay / 60),
    minute: minuteOfDay % 60,
    second: time.second,
    millisecond: time.millisecond
  });
}

function twoDigits(value: number): string {
  return String(value).padStart(2, '0');
}

function threeDigits(value: number): string {
  return String(value).padStart(3, '0');
}

function formatWallClock(time: WallClockTime): string {
  return `${time.year.toString().padStart(4, '0')}-${twoDigits(time.month)}-${twoDigits(time.day)} ${twoDigits(time.hour)}:${twoDigits(time.minute)}:${twoDigits(time.second)}.${threeDigits(time.millisecond)}`;
}

function resultForRange(start: WallClockTime, end: WallClockTime): SkillRunTimeScopeResult {
  if (end.comparisonKey <= start.comparisonKey) {
    return { scope: null, error: '结束时间必须晚于开始时间' };
  }
  if (end.comparisonKey - start.comparisonKey > MAX_SCOPE_MILLIS) {
    return { scope: null, error: '分析时间范围不能超过 24 小时' };
  }
  return {
    scope: {
      start: formatWallClock(start),
      end: formatWallClock(end)
    },
    error: null
  };
}

function parseMinutes(value: string | number | undefined): number | null {
  if (value === undefined || (typeof value === 'string' && !value.trim())) return null;
  const minutes = typeof value === 'number' ? value : Number(value);
  return Number.isSafeInteger(minutes) && minutes >= 0 ? minutes : null;
}

export function toSkillRunTimeScope(input: SkillRunTimeScopeInput): SkillRunTimeScopeResult {
  if (input.mode === 'none') return { scope: null, error: null };

  if (input.mode === 'range') {
    const start = parseWallClock(input.start);
    const end = parseWallClock(input.end);
    if (!start || !end) return { scope: null, error: '请输入完整且有效的开始和结束时间' };
    return resultForRange(start, end);
  }

  const incidentTime = parseWallClock(input.incidentTime);
  const beforeMinutes = parseMinutes(input.beforeMinutes);
  const afterMinutes = parseMinutes(input.afterMinutes);
  if (!incidentTime) return { scope: null, error: '请输入有效的故障时间' };
  if (beforeMinutes === null || afterMinutes === null) {
    return { scope: null, error: '故障前后分钟数必须是非负整数' };
  }
  if (beforeMinutes + afterMinutes > MINUTES_PER_DAY) {
    return { scope: null, error: '分析时间范围不能超过 24 小时' };
  }

  const start = addMinutes(incidentTime, -beforeMinutes);
  const end = addMinutes(incidentTime, afterMinutes);
  if (!start || !end) return { scope: null, error: '请输入完整且有效的时间范围' };
  return resultForRange(start, end);
}
