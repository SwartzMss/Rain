import type { RegistrationSettingField, ResourceMode, SettingCategory } from '../../api/types';

export type SettingFieldGroups = Record<SettingCategory, RegistrationSettingField[]>;


const BYTE_UNITS = [
  { suffix: 'B', multiplier: 1 },
  { suffix: 'K', multiplier: 1024 },
  { suffix: 'M', multiplier: 1024 ** 2 },
  { suffix: 'G', multiplier: 1024 ** 3 },
] as const;

function numericValue(value: unknown): number | null {
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) ? number : null;
}

function formatByteSize(value: unknown): string {
  const number = numericValue(value);
  if (number == null) return String(value ?? '');
  let unit: (typeof BYTE_UNITS)[number] = BYTE_UNITS[0];
  for (const candidate of BYTE_UNITS) {
    if (number >= candidate.multiplier) unit = candidate;
  }
  const amount = number / unit.multiplier;
  return `${amount}${unit.suffix}`;
}

function parseByteSize(value: unknown): number {
  const text = String(value ?? '').trim().toUpperCase();
  const match = /^(\d+(?:\.\d+)?)\s*(B|K|KB|KIB|M|MB|MIB|G|GB|GIB)?$/.exec(text);
  if (!match) throw new Error('容量请输入例如 64M 或 1G 的大小');
  const amount = Number(match[1]);
  const suffix = match[2] ?? 'B';
  const multiplier = suffix.startsWith('G') ? 1024 ** 3
    : suffix.startsWith('M') ? 1024 ** 2
      : suffix.startsWith('K') ? 1024
        : 1;
  const bytes = amount * multiplier;
  if (!Number.isSafeInteger(bytes) || bytes <= 0) {
    throw new Error('容量必须是正整数字节数');
  }
  return bytes;
}

export function groupSettingFields(fields: RegistrationSettingField[]): SettingFieldGroups {
  return {
    common: fields.filter((field) => field.category === 'common'),
    advanced: fields.filter((field) => field.category === 'advanced'),
    expert: fields.filter((field) => field.category === 'expert'),
  };
}

export function recommendedRangeLabel(field: RegistrationSettingField): string | null {
  if (field.recommended_min == null && field.recommended_max == null) return null;
  const format = (value: number | null | undefined, fallback: string) => value == null ? fallback : field.unit === 'bytes' ? formatByteSize(value) : String(value);
  return `推荐 ${format(field.recommended_min, '无下限')}–${format(field.recommended_max, '无上限')}`;
}

export function createSettingDraft(
  fields: RegistrationSettingField[],
  configured: Record<string, unknown> = {},
): Record<string, unknown> {
  return fields.reduce<Record<string, unknown>>((draft, field) => {
    draft[field.key] = configured[field.key]
      ?? field.default_value
      ?? (field.value_type === 'boolean' ? false : field.value_type === 'string_array' ? [] : '');
    return draft;
  }, {});
}

export function settingInputValue(field: RegistrationSettingField, value: unknown): string | number | boolean {
  if (field.unit === 'bytes') return typeof value === 'string' ? value : formatByteSize(value);
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') return Array.isArray(value) ? value.join(', ') : String(value ?? '');
  return value == null ? '' : (value as string | number);
}

export function serializeSettingValue(field: RegistrationSettingField, value: unknown): unknown {
  if (field.unit === 'bytes') return parseByteSize(value);
  if (field.value_type === 'integer') return Number(value);
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') {
    return String(value ?? '').split(',').map((item) => item.trim()).filter(Boolean);
  }
  return value;
}

export function settingUnitLabel(field: RegistrationSettingField): string | null | undefined {
  return field.unit === 'bytes' ? '支持 M / G' : field.unit;
}

export function effectiveSettingLabel(
  configured: unknown,
  effective: unknown,
  pendingRestart: boolean,
): string {
  const suffix = pendingRestart ? '（待重启）' : '';
  return `已配置 ${String(configured)}；当前生效 ${String(effective)}${suffix}`;
}

export function serializeResourceModePatch(
  key: string,
  mode: ResourceMode,
): Record<string, ResourceMode> {
  return { [key]: mode };
}

export function settingHelpText(key: string): string | undefined {
  return {
    temp_results_max_result_size: '一次搜索生成的全部匹配内容及定位信息的总容量；超出后搜索失败。',
    temp_results_max_total_size: '所有临时搜索结果共享的存储配额；配额不足时无法生成新结果。不得小于单次结果容量。',
    temp_results_max_records: '最多保留多少份临时搜索结果，不是匹配行数；达到上限后无法新建结果。',
    temp_results_max_scan_duration_seconds: '文件列表解析与内容扫描分别使用此超时，整个请求可能更久；超时后搜索失败。',
  }[key];
}
