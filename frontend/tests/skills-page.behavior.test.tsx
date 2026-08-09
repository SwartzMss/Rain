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
    reviewSkill: vi.fn(),
    fetchAiProviderStatus: vi.fn()
  }
}));

describe('skills page detail loading', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
  });

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

  it('disables quality assessment when the AI provider is not configured', async () => {
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: false });
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
      id: 'skill-1', name: '诊断', description: 'private', content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    const button = await screen.findByRole('button', { name: '质量评估' });
    expect(button).toBeDisabled();
    expect(screen.getByText('尚未配置 AI 模型服务，暂时无法进行质量评估。')).toBeInTheDocument();
    expect(rainApi.reviewSkill).not.toHaveBeenCalled();
  });
});
