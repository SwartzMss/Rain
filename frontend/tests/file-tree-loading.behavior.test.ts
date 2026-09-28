import { describe, expect, it, vi } from 'vitest';
import { hydrateTreeNode } from '../src/features/files/treeModel';

describe('file tree loading', () => {
  it('keeps an archive retryable when its extracted directory fails to load', async () => {
    const fetchNode = vi.fn()
      .mockResolvedValueOnce({
        node: {
          id: 10,
          parent_id: null,
          name: 'diagnostic.zip',
          path: '/diagnostic.zip',
          is_dir: false,
          preview_kind: 'archive'
        },
        children: [{
          id: 11,
          parent_id: 10,
          name: 'diagnostic.zip_extracted',
          path: '/diagnostic.zip_extracted',
          is_dir: true,
          preview_kind: 'directory'
        }],
        has_more: false,
        next_cursor: null
      })
      .mockRejectedValueOnce(new Error('temporary extraction failure'));

    const result = await hydrateTreeNode('bundle', '10', null, fetchNode);

    expect(result.node.hasLoadedChildren).toBe(false);
    expect(result.node.childrenLoadError).toBe('temporary extraction failure');
    expect(result.children).toEqual([]);
    expect(fetchNode).toHaveBeenCalledTimes(2);

    const retryFetchNode = vi.fn()
      .mockResolvedValueOnce({
        node: {
          id: 10,
          parent_id: null,
          name: 'diagnostic.zip',
          path: '/diagnostic.zip',
          is_dir: false,
          preview_kind: 'archive'
        },
        children: [{
          id: 11,
          parent_id: 10,
          name: 'diagnostic.zip_extracted',
          path: '/diagnostic.zip_extracted',
          is_dir: true,
          preview_kind: 'directory'
        }]
      })
      .mockResolvedValueOnce({
        node: {
          id: 11,
          parent_id: 10,
          name: 'diagnostic.zip_extracted',
          path: '/diagnostic.zip_extracted',
          is_dir: true,
          preview_kind: 'directory'
        },
        children: [{
          id: 12,
          parent_id: 11,
          name: 'application.log',
          path: '/diagnostic.zip_extracted/application.log',
          is_dir: false,
          preview_kind: 'text'
        }]
      });
    const retry = await hydrateTreeNode('bundle', '10', null, retryFetchNode);

    expect(retry.node.hasLoadedChildren).toBe(true);
    expect(retry.node.childrenLoadError).toBeNull();
    expect(retry.children.map((child) => child.name)).toEqual(['application.log']);
  });
});
