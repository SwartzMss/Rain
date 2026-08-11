import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, rainApi } from '../src/api/client';
import { SkillsPage } from '../src/features/skills/SkillsPage';
import { DEFAULT_SKILL_MARKDOWN, REQUIRED_SKILL_SECTIONS } from '../src/features/skills/skillSchema';

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    constructor(message: string, readonly status?: number, readonly code?: string) {
      super(message);
    }
  },
  normalizeApiError: (value: unknown) => value instanceof Error ? value.message : String(value),
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
      schema_version: null, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
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
      schema_version: null, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    const button = await screen.findByRole('button', { name: '质量评估' });
    expect(button).toBeDisabled();
    expect(screen.getByText('尚未配置 AI 模型服务，暂时无法进行质量评估。')).toBeInTheDocument();
    expect(rainApi.reviewSkill).not.toHaveBeenCalled();
  });

  it('prefills a new Skill with the Chinese v1 template and required-section guidance', async () => {
    const user = userEvent.setup();
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([]);

    render(<SkillsPage />);

    await user.click(await screen.findByRole('button', { name: '新建 Skill' }));

    const editor = screen.getByLabelText('SKILL.md');
    expect(editor).toHaveValue(DEFAULT_SKILL_MARKDOWN);
    expect(screen.getAllByLabelText('SKILL.md')).toHaveLength(1);
    expect(screen.getByText('schema_version: 1')).toBeInTheDocument();
    const requiredSections = screen.getByRole('list', { name: '标准中文必填章节' });
    for (const section of REQUIRED_SKILL_SECTIONS) expect(requiredSections).toHaveTextContent(`# ${section}`);
  });

  it('shows the authoritative schema version returned for an existing Skill', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
      id: 'skill-1', name: '诊断', description: 'private', content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      schema_version: 1, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    expect(await screen.findByText('schema_version: 1')).toBeInTheDocument();
  });

  it('marks a historical free-form Skill as requiring v1 migration', async () => {
    const user = userEvent.setup();
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
      id: 'skill-1', name: '旧诊断', description: 'private', content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '旧诊断', description: 'private', skill_markdown: '# Legacy prompt',
      schema_version: null, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    expect(await screen.findByText('schema_version: 未识别（需迁移到 v1）')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '编辑' }));
    expect(screen.getByText('schema_version: 未识别（需迁移到 v1）')).toBeInTheDocument();
    expect(screen.getByLabelText('SKILL.md')).toHaveValue('# Legacy prompt');
  });

  it('keeps the editor open and displays a SKILL_FORMAT_INVALID Chinese message exactly', async () => {
    const user = userEvent.setup();
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([]);
    vi.mocked(rainApi.createSkill).mockRejectedValue(new ApiError('缺少必填章节：证据规则', 400, 'SKILL_FORMAT_INVALID'));

    render(<SkillsPage />);

    await user.click(await screen.findByRole('button', { name: '新建 Skill' }));
    await user.type(screen.getByLabelText('名称'), '蓝牙诊断');
    await user.click(screen.getByRole('button', { name: '保存' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(/^缺少必填章节：证据规则$/);
    expect(screen.getByRole('heading', { name: '新建 Skill' })).toBeInTheDocument();
    expect(rainApi.createSkill).toHaveBeenCalledWith(expect.objectContaining({ skill_markdown: expect.stringContaining('schema_version: 1') }));
  });
});
