import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { IssueSkillRunner } from '../src/features/skill-runs/IssueSkillRunner';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (value: unknown) => String(value),
  rainApi: {
    fetchSkills: vi.fn(),
    fetchAiProviderStatus: vi.fn(),
    fetchActiveSkillRun: vi.fn(),
    createSkillRun: vi.fn(),
    fetchSkillRun: vi.fn(),
    fetchSkillRunResult: vi.fn(),
    cancelSkillRun: vi.fn(),
    skillRunEventsUrl: vi.fn()
  }
}));

describe('issue skill runner', () => {
  beforeEach(() => { vi.clearAllMocks(); sessionStorage.clear(); });

  it('runs an enabled private skill and shows its evidence', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'SUCCEEDED', iteration_count: 1, tool_call_count: 1, cancel_requested: false, created_at: '' });
    vi.mocked(rainApi.fetchSkillRunResult).mockResolvedValue({ summary: { status: 'SUPPORTED', text: '发现数据库超时', evidence_ids: ['e1'] }, observations: [], inferences: [], missing_context: [], evidence: [{ id: 'e1', bundle_hash: 'bundle-a', file_id: 8, path: '/logs/app.log', start_line: 42, end_line: 43, excerpt: 'timeout', explanation: '超时证据' }] });
    const reveal = vi.fn();
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={reveal} />);

    await user.selectOptions(await screen.findByLabelText('选择 Skill'), 'skill-1');
    await user.click(screen.getByRole('button', { name: '运行 Skill' }));
    expect(await screen.findByText('发现数据库超时')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /app\.log:42-43/ }));
    expect(reveal).toHaveBeenCalledWith(expect.objectContaining({ file_id: 8, start_line: 42 }));
  });

  it('shows only enabled Skills', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([
      { id: 'disabled', name: '停用诊断', description: '', schema_version: 1, enabled: false, version: 1, content_hash: 'disabled-hash', created_at: '', updated_at: '', review: null },
      { id: 'enabled', name: '启用诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'enabled-hash', created_at: '', updated_at: '', review: null }
    ]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);

    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    const select = await screen.findByLabelText('选择 Skill');
    expect(select).toHaveValue('enabled');
    expect(screen.queryByRole('option', { name: /停用诊断/ })).not.toBeInTheDocument();
    expect(screen.getByRole('option', { name: /启用诊断/ })).toBeInTheDocument();
    expect(screen.queryByText(/迁移到 v1/)).not.toBeInTheDocument();
  });

  it('offers incident and direct range modes and sends the raw request', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'QUEUED', iteration_count: 0, tool_call_count: 0, cancel_requested: false, created_at: '' });
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    expect(await screen.findByRole('radio', { name: '不限制时间' })).toBeChecked();
    await user.click(screen.getByRole('radio', { name: '直接范围' }));
    expect(screen.getByLabelText('开始时间')).toBeInTheDocument();
    expect(screen.getByLabelText('结束时间')).toBeInTheDocument();
    await user.click(screen.getByRole('radio', { name: '事故时间' }));
    await user.type(screen.getByLabelText('故障时间'), '2026-08-14T09:30');
    await user.clear(screen.getByLabelText('故障前分钟数'));
    await user.type(screen.getByLabelText('故障前分钟数'), '5');
    await user.clear(screen.getByLabelText('故障后分钟数'));
    await user.type(screen.getByLabelText('故障后分钟数'), '10');
    await user.click(screen.getByRole('button', { name: '运行 Skill' }));

    expect(rainApi.createSkillRun).toHaveBeenCalledWith('ISSUE-1', 'skill-1', {
      incident_time: '2026-08-14T09:30',
      before_minutes: 5,
      after_minutes: 10
    });
  });

  it('passes direct ranges through and preserves unrestricted compatibility', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'QUEUED', iteration_count: 0, tool_call_count: 0, cancel_requested: false, created_at: '' });
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    await user.click(await screen.findByRole('radio', { name: '直接范围' }));
    await user.type(screen.getByLabelText('开始时间'), '2026-08-14T10:00');
    await user.type(screen.getByLabelText('结束时间'), '2026-08-14T09:00');
    await user.click(screen.getByRole('button', { name: '运行 Skill' }));
    expect(rainApi.createSkillRun).toHaveBeenCalledWith('ISSUE-1', 'skill-1', {
      start: '2026-08-14T10:00',
      end: '2026-08-14T09:00'
    });
  });

  it('preserves unrestricted compatibility', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'QUEUED', iteration_count: 0, tool_call_count: 0, cancel_requested: false, created_at: '' });
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: '运行 Skill' }));
    expect(rainApi.createSkillRun).toHaveBeenCalledWith('ISSUE-1', 'skill-1', undefined);
  });

  it('disables skill and scope controls while an existing run is active and preserves cancel', async () => {
    const activeRun = { id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'RUNNING' as const, iteration_count: 1, tool_call_count: 1, cancel_requested: false, created_at: '' };
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(activeRun);
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    expect(await screen.findByRole('button', { name: '取消诊断' })).toBeInTheDocument();
    expect(screen.getByLabelText('选择 Skill')).toBeDisabled();
    expect(screen.getByRole('radio', { name: '不限制时间' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '运行 Skill' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '取消诊断' }));
    expect(rainApi.cancelSkillRun).toHaveBeenCalledWith('run-1');
  });

  it('shows the persisted canonical range for a completed scoped run', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '' }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'SUCCEEDED', iteration_count: 1, tool_call_count: 1, cancel_requested: false, created_at: '', analysis_start_time: '2026-08-14 18:01:00', analysis_end_time: '2026-08-14 18:11:00' });

    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    await user.click(screen.getByRole('radio', { name: '事故时间' }));
    await user.type(screen.getByLabelText('故障时间'), '2026-08-14T18:05');
    await user.type(screen.getByLabelText('故障前分钟数'), '4');
    await user.type(screen.getByLabelText('故障后分钟数'), '6');
    await user.click(screen.getByRole('button', { name: '运行 Skill' }));

    expect(rainApi.createSkillRun).toHaveBeenCalledWith('ISSUE-1', 'skill-1', {
      incident_time: '2026-08-14T18:05',
      before_minutes: 4,
      after_minutes: 6
    });
    expect(await screen.findByText('上次运行分析范围：2026-08-14 18:01:00 至 2026-08-14 18:11:00')).toBeInTheDocument();
  });

  it('keeps an unscoped completed run separate from a newly edited incident configuration', async () => {
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', schema_version: 1, enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '' }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.fetchActiveSkillRun).mockResolvedValue(null);
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'SUCCEEDED', iteration_count: 1, tool_call_count: 1, cancel_requested: false, created_at: '' });

    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: '运行 Skill' }));
    expect(await screen.findByText('上次运行分析范围：不限制时间')).toBeInTheDocument();

    await user.click(screen.getByRole('radio', { name: '事故时间' }));
    await user.type(screen.getByLabelText('故障时间'), '2026-08-14T18:05');
    await user.type(screen.getByLabelText('故障前分钟数'), '4');
    await user.type(screen.getByLabelText('故障后分钟数'), '6');

    expect(screen.getByText('上次运行分析范围：不限制时间')).toBeInTheDocument();
    expect(rainApi.createSkillRun).toHaveBeenCalledWith('ISSUE-1', 'skill-1', undefined);
  });
});
