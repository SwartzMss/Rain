import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const [filesView, expressionEditor, searchResultViewer] = await Promise.all([
  readFile(new URL('../src/features/files/FilesView.tsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/features/files/SearchExpressionEditor.tsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/features/files/components/SearchResultViewer.tsx', import.meta.url), 'utf8')
]);

assert.match(filesView, /ariaLabel="日志内容搜索条件"/);
assert.match(filesView, /aria-label="搜索日志内容"/);
assert.match(filesView, /<SearchExpressionEditor/);
const fileSearchEditor = filesView.match(
  /ariaLabel="当前文件搜索条件"[\s\S]*?\/>/
);
assert.ok(fileSearchEditor, 'file search should use the shared token editor');
assert.match(fileSearchEditor[0], /allowOperators=\{false\}/);
const resultFilterEditor = searchResultViewer.match(
  /ariaLabel="当前结果筛选条件"[\s\S]*?\/>/
);
assert.ok(resultFilterEditor, 'result filter should use the shared token editor');
assert.match(resultFilterEditor[0], /allowOperators=\{false\}/);
assert.doesNotMatch(expressionEditor, /简单模式|高级表达式|支持 AND|优先于/);
assert.match(expressionEditor, /SearchTokenEditor/);
assert.match(filesView, /<FileTreeNode/);
const savedSearchControls = filesView.match(
  /<div className="mt-3 flex w-full items-center [^"]*text-xs text-slate-500">[\s\S]*?<SearchExecutionStatus/
);
assert.ok(savedSearchControls, 'saved search controls should be grouped before the execution status');
assert.match(savedSearchControls[0], /justify-(?:end|between)/);
assert.ok(
  savedSearchControls[0].indexOf('我的搜索条件') < savedSearchControls[0].indexOf('保存条件'),
  'my saved searches should appear before the save condition action'
);
assert.doesNotMatch(filesView, /文件名搜索|按文件名|搜索文件或目录/);
assert.doesNotMatch(filesView, /searchMode|filenameQuery|mode: 'filename'/);

console.log('search UI tests passed');
