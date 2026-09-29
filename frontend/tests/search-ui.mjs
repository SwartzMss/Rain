import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const [filesView, expressionEditor] = await Promise.all([
  readFile(new URL('../src/features/files/FilesView.tsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/features/files/SearchExpressionEditor.tsx', import.meta.url), 'utf8')
]);

assert.match(filesView, /ariaLabel="日志内容搜索条件"/);
assert.match(filesView, /aria-label="搜索日志内容"/);
assert.match(filesView, /<SearchExpressionEditor/);
assert.match(expressionEditor, /高级表达式/);
assert.match(filesView, /<FileTreeNode/);
const savedSearchControls = filesView.match(
  /<div className="mt-3 flex flex-wrap items-center [^"]*text-xs text-slate-500">[\s\S]*?<SearchExecutionStatus/
);
assert.ok(savedSearchControls, 'saved search controls should be grouped before the execution status');
assert.match(savedSearchControls[0], /justify-between/);
assert.ok(
  savedSearchControls[0].indexOf('我的搜索条件') < savedSearchControls[0].indexOf('保存条件'),
  'my saved searches should appear before the save condition action'
);
assert.doesNotMatch(filesView, /文件名搜索|按文件名|搜索文件或目录/);
assert.doesNotMatch(filesView, /searchMode|filenameQuery|mode: 'filename'/);

console.log('search UI tests passed');
