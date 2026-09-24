import type { RegistrationSettingField, ResourceMode, SettingCategory } from '../../api/types';

export type SettingFieldGroups = Record<SettingCategory, RegistrationSettingField[]>;

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
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') return Array.isArray(value) ? value.join(', ') : String(value ?? '');
  return value == null ? '' : (value as string | number);
}

export function serializeSettingValue(field: RegistrationSettingField, value: unknown): unknown {
  if (field.value_type === 'integer') return Number(value);
  if (field.value_type === 'boolean') return Boolean(value);
  if (field.value_type === 'string_array') {
    return String(value ?? '').split(',').map((item) => item.trim()).filter(Boolean);
  }
  return value;
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
