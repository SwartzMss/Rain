import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { AiProviderSettingsPanel } from '../src/features/admin/AiProviderSettingsPanel';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (value: unknown) => String(value),
  rainApi: {
    fetchAiProvider: vi.fn(),
    updateAiProvider: vi.fn(),
    testAiProvider: vi.fn()
  }
}));

describe('AI provider settings', () => {
  beforeEach(() => vi.clearAllMocks());

  it('preserves a blank secret while saving and can test the active provider', async () => {
    vi.mocked(rainApi.fetchAiProvider).mockResolvedValue({ configured: true, source: 'DATABASE', base_url: 'https://ai.example/v1', model: 'model-a', request_timeout_seconds: 30, api_key_mask: 'sk-…1234' });
    vi.mocked(rainApi.updateAiProvider).mockResolvedValue({ configured: true, source: 'DATABASE', base_url: 'https://ai.example/v1', model: 'model-a', request_timeout_seconds: 30, api_key_mask: 'sk-…1234' });
    vi.mocked(rainApi.testAiProvider).mockResolvedValue({ ok: true, model: 'model-a' });
    const user = userEvent.setup();
    render(<AiProviderSettingsPanel />);

    expect(await screen.findByDisplayValue('https://ai.example/v1')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '保存 AI 配置' }));
    expect(rainApi.updateAiProvider).toHaveBeenCalledWith({ base_url: 'https://ai.example/v1', model: 'model-a', request_timeout_seconds: 30 });
    await user.click(screen.getByRole('button', { name: '测试连接' }));
    await waitFor(() => expect(screen.getByText('连接成功，模型：model-a')).toBeInTheDocument());
    expect(rainApi.testAiProvider).toHaveBeenCalledWith();
  });

  it('never combines a changed endpoint with the stored secret', async () => {
    vi.mocked(rainApi.fetchAiProvider).mockResolvedValue({ configured: true, source: 'DATABASE', base_url: 'https://ai.example/v1', model: 'model-a', request_timeout_seconds: 30, api_key_mask: 'sk-…1234' });
    const user = userEvent.setup();
    render(<AiProviderSettingsPanel />);

    const baseUrl = await screen.findByDisplayValue('https://ai.example/v1');
    await user.clear(baseUrl);
    await user.type(baseUrl, 'https://other.example/v1');
    await user.click(screen.getByRole('button', { name: '测试连接' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('测试修改后的配置需要重新输入 API Key');
    expect(rainApi.testAiProvider).not.toHaveBeenCalled();
  });
});
