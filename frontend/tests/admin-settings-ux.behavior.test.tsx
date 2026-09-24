import { expect, it } from 'vitest';
import { groupSettingFields, recommendedRangeLabel } from '../src/features/admin/settingsFields';

const commonField = {
  key: 'issue_max_content_size',
  category: 'common' as const,
  visibility: 'default' as const,
};
const advancedField = {
  key: 'upload_concurrent_processing_tasks',
  category: 'advanced' as const,
  visibility: 'collapsed' as const,
  recommended_min: 1,
  recommended_max: 8,
};
const expertField = {
  key: 'argon2_concurrency',
  category: 'expert' as const,
  visibility: 'expert' as const,
};

it('groups metadata fields by backend category', () => {
  expect(groupSettingFields([commonField, advancedField, expertField])).toEqual({
    common: [commonField],
    advanced: [advancedField],
    expert: [expertField],
  });
});

it('formats only advisory recommended ranges', () => {
  expect(recommendedRangeLabel(advancedField)).toBe('推荐 1–8');
  expect(recommendedRangeLabel(commonField)).toBeNull();
});
