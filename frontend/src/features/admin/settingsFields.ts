import type { RegistrationSettingField, SettingCategory } from '../../api/types';

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
