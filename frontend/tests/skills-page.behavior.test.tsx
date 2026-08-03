import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { SkillsPage } from '../src/features/skills/SkillsPage';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (value: unknown) => String(value),
  rainApi: {
    fetchSkills: vi.fn(),
    fetchSkill: vi.fn(),
    createSkill: vi.fn(),
    updateSkill: vi.fn(),
    deleteSkill: vi.fn(),
    reviewSkill: vi.fn()
  }
}));

describe('skills page detail loading', () => {
  beforeEach(() => vi.clearAllMocks());

  it('loads markdown only for the selected skill', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
      id: 'skill-1', name: '诊断', description: 'private', content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    expect(await screen.findByText('# Full private markdown')).toBeInTheDocument();
    expect(rainApi.fetchSkills).toHaveBeenCalledTimes(1);
    expect(rainApi.fetchSkill).toHaveBeenCalledWith('skill-1');
  });
});
