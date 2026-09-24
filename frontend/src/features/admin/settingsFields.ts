import type { RegistrationSettingField, ResourceMode, SettingCategory } from '../../api/types';

export type SettingFieldGroups = Record<SettingCategory, RegistrationSettingField[]>;

const ISSUE_CONTENT_SIZE_KEY = 'issue_max_content_size';
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

function formatIssueContentSize(value: unknown): string {
  const number = numericValue(value);
  if (number == null) return String(value ?? '');
  let unit: (typeof BYTE_UNITS)[number] = BYTE_UNITS[0];
  for (const candidate of BYTE_UNITS) {
    if (number >= candidate.multiplier) unit = candidate;
  }
  const amount = number / unit.multiplier;
  const rounded = Number.isInteger(amount)
    ? amount.toFixed(0)
    : amount.toFixed(amount >= 10 ? 1 : 2).replace(/0+$/, '').replace(/\.$/, '');
  return `${rounded}${unit.suffix}`;
}

function parseIssueContentSize(value: unknown): number {
  const text = String(value ?? '').trim().toUpperCase();
  const match = /^(\d+(?:\.\d+)?)\s*(B|K|KB|KIB|M|MB|MIB|G|GB|GIB)?$/.exec(text);
  if (!match) throw new Error('Issue 内容上限请输入例如 8G 的字节大小');
  const amount = Number(match[1]);
  const suffix = match[2] ?? 'B';
  const multiplier = suffix.startsWith('G') ? 1024 ** 3
    : suffix.startsWith('M') ? 1024 ** 2
      : suffix.startsWith('K') ? 1024
        : 1;
  const bytes = amount * multiplier;
  if (!Number.isSafeInteger(bytes) || bytes <= 0) {
    throw new Error('Issue 内容上限必须是正整数');
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
  return `推荐 ${field.recommended_min ?? '无下限'}–${field.recommended_max ?? '无上限'}`;
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
  if (field.key === ISSUE_CONTENT_SIZE_KEY) return formatIssueContentSize(value);
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') return Array.isArray(value) ? value.join(', ') : String(value ?? '');
  return value == null ? '' : (value as string | number);
}

export function serializeSettingValue(field: RegistrationSettingField, value: unknown): unknown {
  if (field.key === ISSUE_CONTENT_SIZE_KEY) return parseIssueContentSize(value);
  if (field.value_type === 'integer') return Number(value);
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') {
    return String(value ?? '').split(',').map((item) => item.trim()).filter(Boolean);
  }
  return value;
}

export function settingUnitLabel(field: RegistrationSettingField): string | null | undefined {
  return field.key === ISSUE_CONTENT_SIZE_KEY ? 'G' : field.unit;
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
