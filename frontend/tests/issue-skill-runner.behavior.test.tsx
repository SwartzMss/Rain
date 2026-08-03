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
    vi.mocked(rainApi.fetchSkills).mockResolvedValue([{ id: 'skill-1', user_id: 'user-1', name: '诊断', description: '', skill_markdown: '# diagnose', enabled: true, version: 1, content_hash: 'hash', created_at: '', updated_at: '', review: null }]);
    vi.mocked(rainApi.fetchAiProviderStatus).mockResolvedValue({ configured: true });
    vi.mocked(rainApi.createSkillRun).mockResolvedValue({ id: 'run-1', user_id: 'user-1', issue_code: 'ISSUE-1', skill_id: 'skill-1', skill_version: 1, skill_name: '诊断', status: 'SUCCEEDED', iteration_count: 1, tool_call_count: 1, cancel_requested: false, created_at: '' });
    vi.mocked(rainApi.fetchSkillRunResult).mockResolvedValue({ summary: '发现数据库超时', observations: [], inferences: [], missing_context: [], evidence: [{ file_id: 8, path: '/logs/app.log', start_line: 42, end_line: 43, excerpt: 'timeout', explanation: '超时证据' }] });
    const reveal = vi.fn();
    const user = userEvent.setup();
    render(<IssueSkillRunner issueCode="ISSUE-1" onRevealEvidence={reveal} />);

    await user.selectOptions(await screen.findByLabelText('选择 Skill'), 'skill-1');
    await user.click(screen.getByRole('button', { name: '运行 Skill' }));
    expect(await screen.findByText('发现数据库超时')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /app\.log:42-43/ }));
    expect(reveal).toHaveBeenCalledWith(expect.objectContaining({ file_id: 8, start_line: 42 }));
  });
});
