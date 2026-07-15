import { rainApi } from '../../../api/client';
import type { FileRow } from '../homeRows';
import { formatBytes, stageClass, stageLabel } from '../homeRows';

type UploadFileTableProps = {
  bundlesError: string | null;
  currentIssueCode: string;
  deletingKey: string | null;
  fileRows: FileRow[];
  onDeleteRow: (row: FileRow) => void;
};

export function UploadFileTable({
  bundlesError,
  currentIssueCode,
  deletingKey,
  fileRows,
  onDeleteRow
}: UploadFileTableProps) {
  return (
    <div className="rounded-lg border border-slate-200 bg-white p-4">
      <div className="mb-4 flex items-center justify-between gap-3">
        <h3 className="text-lg font-semibold text-slate-950">文件列表</h3>
        {bundlesError ? <span className="text-sm text-rose-600">{bundlesError}</span> : null}
      </div>
      <div className="overflow-x-auto rounded-lg border border-slate-200">
        <table className="min-w-full divide-y divide-slate-200 text-sm">
          <thead className="bg-slate-50 text-left text-xs uppercase text-slate-500">
            <tr>
              <th className="px-4 py-2.5 font-medium">文件名</th>
              <th className="px-4 py-2.5 font-medium">状态</th>
              <th className="px-4 py-2.5 font-medium">大小</th>
              <th className="px-4 py-2.5 font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-200 text-slate-700">
            {fileRows.map((row) => {
              const deleting = deletingKey === row.key;
              return (
                <tr key={row.key}>
                  <td className="max-w-[360px] truncate px-4 py-3">
                    <span className="mr-2 text-slate-500">□</span>
                    {row.name}
                  </td>
                  <td className="px-4 py-3">
                    <span className={`rounded-full border px-2 py-1 text-xs ${stageClass(row.stage)}`}>
                      {stageLabel(row.stage, row.progressPercent)}
                    </span>
                    {row.failureReason ? (
                      <p className="mt-1 max-w-[360px] text-xs text-rose-600">{row.failureReason}</p>
                    ) : null}
                  </td>
                  <td className="px-4 py-3">{formatBytes(row.sizeBytes)}</td>
                  <td className="whitespace-nowrap px-4 py-3">
                    {row.status === 'READY' && row.file ? (
                      <a
                        className="mr-4 text-sky-700 hover:text-sky-800"
                        href={rainApi.fileDownloadUrl(row.bundleHash, String(row.file.id))}
                      >
                        下载
                      </a>
                    ) : null}
                    {row.status === 'PROCESSING' || row.status === 'PENDING' ? (
                      <span className="mr-4 text-slate-500">等待完成</span>
                    ) : null}
                    {row.stage !== 'UPLOADING' && row.bundleHash ? (
                      <button
                        type="button"
                        className="text-rose-600 hover:text-rose-700 disabled:text-slate-600"
                        disabled={deleting || row.status === 'PROCESSING' || row.status === 'PENDING'}
                        onClick={() => onDeleteRow(row)}
                      >
                        {deleting ? '删除中...' : '删除'}
                      </button>
                    ) : null}
                  </td>
                </tr>
              );
            })}
            {!fileRows.length ? (
              <tr>
                <td colSpan={4} className="px-4 py-10 text-center text-slate-500">
                  {currentIssueCode ? '暂无文件' : '请选择一个 Issue'}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    </div>
  );
}
