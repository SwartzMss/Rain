import { rainApi } from '../../../api/client';
import { useAuth } from '../../../auth/AuthContext';
import type { FileRow } from '../homeRows';
import { canDeleteFileRow, formatBytes, stageClass, stageLabel } from '../homeRows';
import { FileIcon } from './FileIcons';

type UploadFileTableProps = {
  bundlesError: string | null;
  currentIssueCode: string;
  deletingKey: string | null;
  deletingKeys?: ReadonlySet<string>;
  selectedRowKeys?: ReadonlySet<string>;
  fileRows: FileRow[];
  canWrite: boolean;
  onDeleteRow: (row: FileRow) => void;
  onToggleRow?: (row: FileRow) => void;
  onToggleAll?: (checked: boolean, rows: FileRow[]) => void;
  onClearSelection?: () => void;
  onDeleteSelected?: () => void;
  onRetryUpload: (taskId: string) => void;
};

export function UploadFileTable({
  bundlesError,
  currentIssueCode,
  deletingKey,
  deletingKeys,
  selectedRowKeys,
  fileRows,
  canWrite,
  onDeleteRow,
  onToggleRow,
  onToggleAll,
  onClearSelection,
  onDeleteSelected,
  onRetryUpload
}: UploadFileTableProps) {
  const auth = useAuth();
  const canDownload = auth.state.status === 'AUTHENTICATED';
  const selectableRows = fileRows.filter((row) => canWrite && canDeleteFileRow(row));
  const selectedCount = selectedRowKeys?.size ?? 0;
  const allSelected = selectableRows.length > 0 && selectableRows.every((row) => selectedRowKeys?.has(row.key));

  return (
    <div className="rounded-2xl border border-slate-200/90 bg-white/95 p-4 shadow-[0_18px_48px_rgba(7,21,34,0.08)] backdrop-blur">
      <div className="mb-4 flex items-center justify-between gap-3">
        <h3 className="text-lg font-semibold text-slate-950">文件列表</h3>
        <div className="flex items-center gap-3">
          {selectedCount ? (
            <>
              <span className="text-sm text-slate-600">已选 {selectedCount} 项</span>
              {onClearSelection ? (
                <button type="button" className="text-sm text-slate-600 hover:text-slate-950" onClick={onClearSelection}>
                  取消选择
                </button>
              ) : null}
              {onDeleteSelected ? (
                <button
                  type="button"
                  className="rounded-lg bg-rose-500 px-3 py-2 text-sm font-semibold text-white transition hover:bg-rose-600 disabled:opacity-60"
                  onClick={onDeleteSelected}
                  disabled={!!deletingKeys?.size}
                >
                  删除所选
                </button>
              ) : null}
            </>
          ) : null}
          {bundlesError ? <span className="text-sm text-rose-600">{bundlesError}</span> : null}
        </div>
      </div>
      <div className="overflow-x-auto rounded-lg border border-slate-200">
        <table className="min-w-full divide-y divide-slate-200 text-sm">
          <thead className="bg-slate-50 text-left text-xs uppercase text-slate-500">
            <tr>
              <th className="w-10 px-4 py-2.5 font-medium">
                <input
                  type="checkbox"
                  aria-label="全选可删除文件"
                  checked={allSelected}
                  disabled={!selectableRows.length || !!deletingKeys?.size}
                  onChange={(event) => onToggleAll?.(event.target.checked, selectableRows)}
                />
              </th>
              <th className="px-4 py-2.5 font-medium">文件名</th>
              <th className="px-4 py-2.5 font-medium">状态</th>
              <th className="px-4 py-2.5 font-medium">大小</th>
              <th className="px-4 py-2.5 font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-200 text-slate-700">
            {fileRows.map((row) => {
              const deleting = deletingKey === row.key || deletingKeys?.has(row.key);
              const selectable = canWrite && canDeleteFileRow(row);
              return (
                <tr key={row.key} className="transition hover:bg-slate-50/80">
                  <td className="px-4 py-3">
                    <input
                      type="checkbox"
                      aria-label={`选择 ${row.name}`}
                      checked={selectedRowKeys?.has(row.key) ?? false}
                      disabled={!selectable || !!deletingKeys?.size}
                      onChange={() => onToggleRow?.(row)}
                    />
                  </td>
                  <td className="max-w-[360px] truncate px-4 py-3">
                    <FileIcon name={row.name} className="mr-2 inline-block align-middle" />
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
                    {row.uploadTaskId && (row.stage === 'FAILED' || row.stage === 'UNCONFIRMED') ? (
                      <button
                        type="button"
                        className="mr-4 text-sky-700 hover:text-sky-800"
                        onClick={() => onRetryUpload(row.uploadTaskId!)}
                      >
                        重试
                      </button>
                    ) : null}
                    {canDownload && row.status === 'READY' && row.file ? (
                      <a
                        className="mr-4 text-sky-700 hover:text-sky-800"
                        href={rainApi.fileDownloadUrl(row.bundleHash, String(row.file.id))}
                      >
                        下载
                      </a>
                    ) : null}
                    {row.stage === 'QUEUED' ? (
                      <span role="status" className="mr-4 text-slate-600">等待上传</span>
                    ) : null}
                    {row.stage === 'RETRY_WAIT' ? (
                      <span role="status" className="mr-4 text-amber-700">等待重试</span>
                    ) : null}
                    {row.stage === 'UPLOADING' || row.stage === 'ACCEPTED' || row.status === 'PROCESSING' || row.status === 'PENDING' ? (
                      <span role="status" className="mr-4 text-amber-700">处理中，暂不可删除</span>
                    ) : null}
                    {canDeleteFileRow(row) && canWrite ? (
                      <button
                        type="button"
                        className="text-rose-600 hover:text-rose-700 disabled:text-slate-600"
                        disabled={deleting}
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
                <td colSpan={5} className="px-4 py-10 text-center text-slate-500">
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
