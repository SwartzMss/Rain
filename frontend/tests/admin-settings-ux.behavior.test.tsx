import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { AdminSettingsPage } from '../src/features/admin/AdminPage';
import { rainApi } from '../src/api/client';
import {
  effectiveSettingLabel,
  groupSettingFields,
  recommendedRangeLabel,
  serializeResourceModePatch,
} from '../src/features/admin/settingsFields';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: {
    me: vi.fn(),
    fetchAdminSettings: vi.fn(),
    updateAdminSettings: vi.fn(),
    updateAdminSettingsV2: vi.fn(),
  },
}));

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

it('formats configured and effective values separately', () => {
  expect(effectiveSettingLabel(4, 2, true)).toBe('已配置 4；当前生效 2（待重启）');
  expect(effectiveSettingLabel(4, 4, false)).toBe('已配置 4；当前生效 4');
});

it('serializes resource mode patches without changing legacy values', () => {
  expect(serializeResourceModePatch('upload_concurrent_processing_tasks', 'auto'))
    .toEqual({ upload_concurrent_processing_tasks: 'auto' });
});

it('presents metadata settings by visibility and reveals expert settings explicitly', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({
    authenticated: true,
    user: { id: 'admin', username: 'admin', role: 'ADMIN' },
  });
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce({
    allow_registration: true,
    updated_at: '',
    updated_by_username: 'admin',
    login_ip_limit_per_minute: 20,
    login_username_failure_limit_per_5_minutes: 10,
    issue_inactive_days: 0,
    cleanup_exempt_usernames: [],
    revision: '7',
    configured: {
      issue_max_content_size: 8 * 1024 * 1024 * 1024,
      upload_concurrent_processing_tasks: 4,
      argon2_concurrency: 5,
    },
    fields: [
      {
        key: 'issue_max_content_size', category: 'common', visibility: 'default',
        value_type: 'integer', unit: 'bytes', description: '单个 Issue 的内容总量上限',
        default_value: 8 * 1024 * 1024 * 1024, min: 1, max: null,
        apply_mode: 'hot', db_column: 'issue_max_content_size', env_name: 'RAIN_ISSUE_MAX_CONTENT_SIZE',
        recommended_min: 1, recommended_max: 32,
      },
      {
        key: 'upload_concurrent_processing_tasks', category: 'advanced', visibility: 'collapsed',
        value_type: 'integer', unit: 'tasks', description: '上传处理并发数',
        default_value: 4, min: 1, max: null,
        apply_mode: 'restart_required', db_column: 'upload_concurrent_processing_tasks', env_name: 'RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS',
        recommended_min: 1, recommended_max: 8,
      },
      {
        key: 'argon2_concurrency', category: 'expert', visibility: 'expert',
        value_type: 'integer', unit: 'tasks', description: 'Argon2 并发计算数',
        default_value: 5, min: 1, max: null,
        apply_mode: 'restart_required', db_column: 'argon2_concurrency', env_name: 'RAIN_AUTH_ARGON2_CONCURRENCY',
        recommended_min: 1, recommended_max: 16,
      },
    ],
    runtime: {
      policy_version: 'v1',
      resources: {
        cpu_cores: 1,
        memory_limit_bytes: 512 * 1024 * 1024,
        cpu_source: 'fallback',
        memory_source: 'fallback',
        warnings: ['cpu_probe_fallback', 'memory_probe_fallback'],
      },
      upload_concurrent_processing_tasks: 1,
      search_tantivy_max_writers: 1,
      search_tantivy_writer_heap_size: 16 * 1024 * 1024,
      estimated_bytes: 112 * 1024 * 1024,
      adaptive_memory_target_bytes: 128 * 1024 * 1024,
      warnings: ['adaptive_memory_target_is_heuristic'],
      decisions: [],
    },
  } as never);

  render(
    <MemoryRouter initialEntries={['/admin/settings']}>
      <AuthProvider><AdminSettingsPage /></AuthProvider>
    </MemoryRouter>,
  );

  await waitFor(() => expect(screen.getByText('常用配置')).toBeInTheDocument());
  expect(screen.getByTestId('runtime-resource-summary')).toBeInTheDocument();
  expect(screen.getAllByText(/fallback（探测不可用）/)).toHaveLength(2);
  expect(screen.getByText(/自适应内存目标是启发式估算/)).toBeInTheDocument();
  expect(screen.getByText('高级运行参数')).toBeInTheDocument();
  expect(screen.queryByText('专家配置')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: '显示专家配置' })).toBeInTheDocument();
  expect(screen.getByText(/推荐 1–32/)).toBeInTheDocument();
  expect(screen.getAllByText('即时生效').length).toBeGreaterThan(0);
  expect(screen.getByText('重启生效')).toBeInTheDocument();

  const advancedDetails = screen.getByText('高级运行参数').closest('details');
  expect(advancedDetails).not.toBeNull();
  expect(advancedDetails).not.toHaveAttribute('open');

  await userEvent.click(screen.getByRole('button', { name: '显示专家配置' }));
  expect(screen.getByText('专家配置')).toBeInTheDocument();
  expect(screen.getByText(/Argon2 并发计算数/)).toBeInTheDocument();

  vi.mocked(rainApi.updateAdminSettingsV2).mockResolvedValueOnce({
    revision: '8',
    configured: { upload_concurrent_processing_tasks: 6 },
    pending_restart_fields: ['upload_concurrent_processing_tasks'],
  } as never);
  await userEvent.click(screen.getByText('高级运行参数'));
  const processingTasks = screen.getByLabelText('upload_concurrent_processing_tasks');
  await userEvent.clear(processingTasks);
  await userEvent.type(processingTasks, '6');
  await userEvent.click(screen.getByRole('button', { name: '保存高级运行参数' }));

  await waitFor(() => {
    expect(rainApi.updateAdminSettingsV2).toHaveBeenCalledWith('7', {
      upload_concurrent_processing_tasks: 6,
    });
  });
  expect(screen.getByText('高级运行参数已保存')).toBeInTheDocument();
});

it('renders resource modes and submits a manual mode change with its value', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({
    authenticated: true,
    user: { id: 'admin', username: 'admin', role: 'ADMIN' },
  });
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce({
    allow_registration: true,
    updated_at: '',
    updated_by_username: 'admin',
    login_ip_limit_per_minute: 20,
    login_username_failure_limit_per_5_minutes: 10,
    issue_inactive_days: 0,
    cleanup_exempt_usernames: [],
    revision: '7',
    configured: { upload_concurrent_processing_tasks: 4 },
    effective: { upload_concurrent_processing_tasks: 2 },
    resource_modes: { upload_concurrent_processing_tasks: 'auto' },
    auto_values: { upload_concurrent_processing_tasks: 4 },
    security: { argon2id_enabled: true },
    pending_restart_fields: ['upload_concurrent_processing_tasks'],
    fields: [{
      key: 'upload_concurrent_processing_tasks',
      category: 'advanced',
      visibility: 'collapsed',
      value_type: 'integer',
      unit: 'tasks',
      description: '上传处理并发数',
      default_value: 4,
      min: 1,
      max: 8,
      apply_mode: 'restart_required',
      db_column: 'upload_concurrent_processing_tasks',
      env_name: 'RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS',
      supports_auto: true,
      auto_value: 4,
    }],
  } as never);
  vi.mocked(rainApi.updateAdminSettingsV2).mockResolvedValueOnce({
    revision: '8',
    configured: { upload_concurrent_processing_tasks: 3 },
    effective: { upload_concurrent_processing_tasks: 2 },
    resource_modes: { upload_concurrent_processing_tasks: 'manual' },
    pending_restart_fields: ['upload_concurrent_processing_tasks'],
  } as never);

  render(
    <MemoryRouter initialEntries={['/admin/settings']}>
      <AuthProvider><AdminSettingsPage /></AuthProvider>
    </MemoryRouter>,
  );

  expect(await screen.findByText('Argon2id 已启用')).toBeInTheDocument();
  await userEvent.click(screen.getByText('高级运行参数'));
  expect(screen.getByLabelText('upload_concurrent_processing_tasks mode')).toHaveValue('auto');
  expect(screen.getByText('已配置 4；当前生效 2（待重启）')).toBeInTheDocument();
  const processingTasks = screen.getByLabelText('upload_concurrent_processing_tasks');
  expect(processingTasks).toBeDisabled();

  await userEvent.selectOptions(screen.getByLabelText('upload_concurrent_processing_tasks mode'), 'manual');
  expect(processingTasks).not.toBeDisabled();
  await userEvent.clear(processingTasks);
  await userEvent.type(processingTasks, '3');
  await userEvent.click(screen.getByRole('button', { name: '保存高级运行参数' }));

  await waitFor(() => {
    expect(rainApi.updateAdminSettingsV2).toHaveBeenCalledWith('7', {
      upload_concurrent_processing_tasks: 3,
    }, {
      upload_concurrent_processing_tasks: 'manual',
    });
  });
});
