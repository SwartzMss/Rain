import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const filesView = await readFile(
  new URL('../src/features/files/FilesView.tsx', import.meta.url),
  'utf8'
);

assert.match(filesView, /ariaLabel="日志内容搜索条件"/);
assert.match(filesView, /aria-label="搜索日志内容"/);
assert.match(filesView, /<FileTreeNode/);
assert.doesNotMatch(filesView, /文件名搜索|按文件名|搜索文件或目录/);
assert.doesNotMatch(filesView, /searchMode|filenameQuery|mode: 'filename'/);

console.log('search UI tests passed');
