import { memo } from 'react';
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import css from 'highlight.js/lib/languages/css';
import go from 'highlight.js/lib/languages/go';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';

hljs.registerLanguage('bash', bash);
hljs.registerLanguage('css', css);
hljs.registerLanguage('go', go);
hljs.registerLanguage('java', java);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('json', json);
hljs.registerLanguage('markdown', markdown);
hljs.registerLanguage('python', python);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('yaml', yaml);

const extensionLanguages: Record<string, string> = {
  bash: 'bash', sh: 'bash', zsh: 'bash', ps1: 'bash',
  css: 'css', scss: 'css', less: 'css',
  go: 'go',
  java: 'java',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  json: 'json', jsonc: 'json',
  md: 'markdown', markdown: 'markdown',
  py: 'python',
  rs: 'rust',
  sql: 'sql',
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  html: 'xml', htm: 'xml', xml: 'xml', svg: 'xml', vue: 'xml',
  yaml: 'yaml', yml: 'yaml'
};

const exactNameLanguages: Record<string, string> = {
  dockerfile: 'bash',
  makefile: 'bash',
  '.bashrc': 'bash',
  '.zshrc': 'bash'
};

export function getSyntaxLanguage(fileName: string): string | null {
  const normalized = fileName.trim().toLowerCase();
  const exact = exactNameLanguages[normalized];
  if (exact) return exact;
  const dot = normalized.lastIndexOf('.');
  if (dot < 0 || dot === normalized.length - 1) return null;
  return extensionLanguages[normalized.slice(dot + 1)] ?? null;
}

export const SyntaxHighlightedLine = memo(function SyntaxHighlightedLine({
  content,
  language
}: {
  content: string;
  language: string;
}) {
  // Avoid spending disproportionate CPU on generated/minified single-line files.
  if (content.length > 20_000) return <>{content}</>;
  const html = hljs.highlight(content, { language, ignoreIllegals: true }).value;
  return <span className="hljs" dangerouslySetInnerHTML={{ __html: html }} />;
});
