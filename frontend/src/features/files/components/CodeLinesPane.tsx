import React from 'react';
import type { FileLine } from '../../../api/types';

type CodeLinesPaneProps = {
  lines: FileLine[];
  lineNumberOffset?: number;
  contentRef?: React.RefObject<HTMLDivElement>;
  renderLine?: (line: FileLine, index: number) => React.ReactNode;
  className?: string;
};

export function CodeLinesPane({
  lines,
  lineNumberOffset = 0,
  contentRef,
  renderLine,
  className = ''
}: CodeLinesPaneProps) {
  return (
    <div
      ref={contentRef}
      className={`min-h-[70vh] flex-1 overflow-auto bg-white p-0 text-xs leading-5 text-slate-900 ${className}`}
    >
      <div className="grid min-h-full grid-cols-[58px_1fr] font-mono">
        <div className="select-none border-r border-slate-100 bg-slate-50 px-3 py-3 text-right text-slate-500">
          {lines.map((line, index) => (
            <div key={`line-number:${line.line_number}:${index}`}>
              {lineNumberOffset + index + 1}
            </div>
          ))}
        </div>
        <div className="px-4 py-3">
          {lines.map((line, index) => (
            <div key={`line-content:${line.line_number}:${index}`} className="whitespace-pre">
              {renderLine ? renderLine(line, index) : line.content}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
