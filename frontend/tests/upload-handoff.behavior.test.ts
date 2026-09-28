import { describe, expect, it } from 'vitest';
import type { UploadResponse, UploadSummary } from '../src/api/types';
import { buildFileRows } from '../src/features/files/homeRows';
import { createOptimisticUploadRows, type UploadTaskSnapshot } from '../src/features/files/uploadRows';

const response: UploadResponse = {
  task_id: 'task-226',
  issue_code: 'ISSUE-226',
  bundle_hash: 'bundle-226',
  status: 'FAILED',
  stage: 'FAILED',
  file_count: 0,
  total_bytes: 1
};

const handedOffTask: UploadTaskSnapshot = {
  id: 'task-226',
  issueCode: 'ISSUE-226',
  file: new File(['broken'], 'broken.zip'),
  name: 'broken.zip',
  sizeBytes: 7,
  status: 'HANDED_OFF',
  progressPercent: 100,
  message: null,
  response
};

const readyBundle: UploadSummary = {
  hash: 'bundle-226',
  name: 'broken.zip',
  status: { upload_status: 'READY' },
  stage: 'READY',
  size_bytes: 7
};

describe('upload handoff row behavior', () => {
  it('does not create an optimistic row for a handed-off task when its Bundle is gone', () => {
    expect(createOptimisticUploadRows([handedOffTask], new Set())).toEqual([]);
    expect(buildFileRows({ bundles: [], bundleFiles: {}, uploadTasks: [handedOffTask] })).toEqual([]);
  });

  it('keeps a handed-off task out of rows while the server READY Bundle remains visible', () => {
    const rows = buildFileRows({
      bundles: [readyBundle],
      bundleFiles: {
        'bundle-226': {
          files: [{
            id: 1,
            name: 'app.log',
            path: 'app.log',
            is_dir: false,
            preview_kind: 'text',
            size_bytes: 1
          }],
          loading: false,
          loaded: true,
          error: null
        }
      },
      uploadTasks: [handedOffTask]
    });

    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ bundleHash: 'bundle-226', name: 'app.log', stage: 'READY' });
  });
});
