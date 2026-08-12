import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, rainApi } from '../src/api/client';
import { SkillsPage } from '../src/features/skills/SkillsPage';
import { DEFAULT_SKILL_MARKDOWN, REQUIRED_SKILL_SECTIONS } from '../src/features/skills/skillSchema';

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const existingReview = {
  overall_score: 86,
  grade: '良好',
  dimensions: { 完整性: 88, 可执行性: 84 },
  warnings: [],
  suggestions: ['补充失败场景'],
  evaluated_at: '2026-08-12T00:00:00Z'
};

function mockSingleSkill(review: typeof existingReview | null) {
  vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
    id: 'skill-1', name: '诊断', description: 'private', schema_version: 1, content_hash: 'hash',
    enabled: true, version: 1, created_at: '', updated_at: '', review
  }]);
  vi.mocked(rainApi.fetchSkill).mockResolvedValue({
    id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
    schema_version: 1, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review
  });
}

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
      id: 'skill-1', name: '诊断', description: 'private', schema_version: 1, content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      schema_version: 1, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    expect(await screen.findByText('# Full private markdown')).toBeInTheDocument();
    expect(rainApi.fetchSkills).toHaveBeenCalledTimes(1);
    expect(rainApi.fetchSkill).toHaveBeenCalledWith('skill-1');
  });

  it('disables quality assessment when the AI provider is not configured', async () => {
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: false });
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{
      id: 'skill-1', name: '诊断', description: 'private', schema_version: 1, content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      schema_version: 1, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    const button = await screen.findByRole('button', { name: '质量评估' });
    expect(button).toBeDisabled();
    expect(screen.getByText('尚未配置 AI 模型服务，暂时无法进行质量评估。')).toBeInTheDocument();
    expect(rainApi.reviewSkill).not.toHaveBeenCalled();
  });

  it('replaces an empty review with explicit feedback while the first assessment is pending', async () => {
    const user = userEvent.setup();
    const pending = deferred<typeof existingReview>();
    mockSingleSkill(null);
    vi.mocked(rainApi.reviewSkill).mockReturnValue(pending.promise);

    render(<SkillsPage />);

    await user.click(await screen.findByRole('button', { name: '质量评估' }));

    expect(screen.getByRole('button', { name: 'AI 评估中...' })).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('正在评估，请稍候…');
    expect(screen.queryByText('当前版本尚未评估。')).not.toBeInTheDocument();
  });

  it('hides the previous score while reassessment is pending and restores it on failure', async () => {
    const user = userEvent.setup();
    const pending = deferred<typeof existingReview>();
    mockSingleSkill(existingReview);
    vi.mocked(rainApi.reviewSkill).mockReturnValue(pending.promise);

    render(<SkillsPage />);

    expect(await screen.findByText('86')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '质量评估' }));

    expect(screen.queryByText('86')).not.toBeInTheDocument();
    expect(screen.queryByText('86 分')).not.toBeInTheDocument();
    expect(screen.getByText(/评估中$/)).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('正在评估，请稍候…');

    pending.reject(new Error('AI 服务暂时不可用'));

    expect(await screen.findByRole('alert')).toHaveTextContent('AI 服务暂时不可用');
    expect(screen.getByText('86')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '质量评估' })).toBeEnabled();
  });

  it('refreshes the Skill detail and exits the loading state after assessment succeeds', async () => {
    const user = userEvent.setup();
    const pending = deferred<typeof existingReview>();
    mockSingleSkill(null);
    vi.mocked(rainApi.reviewSkill).mockReturnValue(pending.promise);

    render(<SkillsPage />);

    await user.click(await screen.findByRole('button', { name: '质量评估' }));
    pending.resolve(existingReview);

    expect(await screen.findByRole('button', { name: '质量评估' })).toBeEnabled();
    expect(screen.getByText('86')).toBeInTheDocument();
    expect(rainApi.fetchSkills).toHaveBeenCalledTimes(1);
    expect(rainApi.fetchSkill).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
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
      id: 'skill-1', name: '诊断', description: 'private', schema_version: 1, content_hash: 'hash',
      enabled: true, version: 1, created_at: '', updated_at: '', review: null
    }]);
    vi.mocked(rainApi.fetchSkill).mockResolvedValue({
      id: 'skill-1', name: '诊断', description: 'private', skill_markdown: '# Full private markdown',
      schema_version: 1, content_hash: 'hash', enabled: true, version: 1, created_at: '', updated_at: '', review: null
    });

    render(<SkillsPage />);

    expect(await screen.findByText('schema_version: 1')).toBeInTheDocument();
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
