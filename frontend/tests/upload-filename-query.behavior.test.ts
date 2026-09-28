import { afterEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';

describe('upload filename metadata', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('sends the original filename with a single-file upload', async () => {
    const response = {
      task_id: 'bundle-hash',
      issue_code: 'ISSUE-1',
      bundle_hash: 'bundle-hash',
      status: 'PROCESSING',
      stage: 'RECEIVING',
      file_count: 1,
      total_bytes: 1
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(response), { status: 202 })
    );
    vi.stubGlobal('fetch', fetchMock);

    await rainApi.uploadLogs('ISSUE-1', [new File(['data'], '车辆日志.zip')]);

    expect(fetchMock.mock.calls[0][0]).toBe(
      '/api/issues/ISSUE-1/uploads?file_name=%E8%BD%A6%E8%BE%86%E6%97%A5%E5%BF%97.zip'
    );
  });
});
