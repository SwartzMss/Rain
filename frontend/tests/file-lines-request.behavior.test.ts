import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';

describe('file lines request cancellation', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('passes AbortSignal to fetch while keeping pagination in the URL', async () => {
    const response = {
      ok: true,
      text: vi.fn().mockResolvedValue(JSON.stringify({
        path: 'app.log',
        start: 10,
        limit: 5000,
        lines: []
      })),
      headers: new Headers()
    } as unknown as Response;
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(response);
    const controller = new AbortController();

    await rainApi.fetchFileLines('bundle/a', 'file/1', {
      start: 10,
      limit: 10000,
      signal: controller.signal
    });

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/files/v1/bundle%2Fa/files/file%2F1/lines?start=10&limit=10000',
      expect.objectContaining({ signal: controller.signal })
    );
  });
});
