import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { uploadFailureMessage } = await server.ssrLoadModule(
    '/src/features/files/uploadFailure.ts'
  );

  assert.equal(
    uploadFailureMessage({ status: 'FAILED', failure_reason: '压缩包条目超过上限' }),
    '压缩包条目超过上限'
  );
  assert.equal(
    uploadFailureMessage({ status: 'FAILED', failure_reason: null }),
    '上传处理失败，请删除后重试'
  );
  assert.equal(uploadFailureMessage({ status: 'PROCESSING', failure_reason: 'ignored' }), null);
} finally {
  await server.close();
}

console.log('upload failure tests passed');
