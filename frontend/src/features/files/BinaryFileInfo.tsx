import type { FileNode } from '../../api/types';

type BinaryFileInfoProps = {
  node: Pick<FileNode, 'name' | 'mime_type' | 'size_bytes'>;
};

const formatSize = (bytes?: number) => {
  if (bytes === undefined || bytes === null) return '未知';
  const units = ['B', 'KB', 'MB', 'GB'];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? size.toFixed(0) : size.toFixed(1)} ${units[unit]}`;
};

export function BinaryFileInfo({ node }: BinaryFileInfoProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label="二进制文件信息">
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-slate-200 pb-5">
        <div className="flex min-w-0 items-center gap-3">
          <span className="flex h-10 w-12 shrink-0 items-center justify-center rounded border border-amber-500/40 bg-amber-500/10 text-xs font-semibold text-amber-700">
            BIN
          </span>
          <div className="min-w-0">
            <h2 className="break-all text-base font-semibold text-slate-950">{node.name}</h2>
            <p className="mt-1 text-xs text-slate-500">二进制文件不支持文字预览</p>
          </div>
        </div>
      </div>

      <dl className="mt-5 divide-y divide-slate-200 border-y border-slate-200 text-sm">
        <div className="grid grid-cols-[120px_minmax(0,1fr)] gap-4 py-3">
          <dt className="text-slate-500">文件名</dt>
          <dd className="break-all text-slate-700">{node.name}</dd>
        </div>
        <div className="grid grid-cols-[120px_minmax(0,1fr)] gap-4 py-3">
          <dt className="text-slate-500">文件类型</dt>
          <dd className="break-all text-slate-700">{node.mime_type || 'application/octet-stream'}</dd>
        </div>
        <div className="grid grid-cols-[120px_minmax(0,1fr)] gap-4 py-3">
          <dt className="text-slate-500">文件大小</dt>
          <dd className="text-slate-700">{formatSize(node.size_bytes)}</dd>
        </div>
      </dl>
    </section>
  );
}
