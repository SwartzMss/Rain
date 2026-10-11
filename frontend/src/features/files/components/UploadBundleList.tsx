import type { UploadSummary } from '../../../api/types';
import { canDeleteFileRow, stageClass, stageLabel } from '../homeRows';

type UploadBundleListProps = {
  bundles: UploadSummary[];
  canWrite: boolean;
  deletingKeys: ReadonlySet<string>;
  highlightDelete?: boolean;
  onDeleteBundle: (bundle: UploadSummary) => void;
};

export function UploadBundleList({ bundles, canWrite, deletingKeys, highlightDelete = false, onDeleteBundle }: UploadBundleListProps) {
  if (!bundles.length) return null;

  const deletableBundles = bundles.filter((bundle) => canDeleteFileRow({
    key: bundle.hash,
    bundleHash: bundle.hash,
    bundleName: bundle.name || bundle.hash,
    name: bundle.name || bundle.hash,
    status: bundle.status.upload_status,
    stage: bundle.stage
  }));

  return (
    <section aria-label="已上传压缩包" className="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-lg font-semibold text-slate-950">已上传压缩包</h3>
        <p className="text-xs text-slate-500">删除压缩包会移除其中的全部日志文件。</p>
      </div>
      <ul className="space-y-2">
        {bundles.map((bundle) => {
          const row = {
            key: bundle.hash,
            bundleHash: bundle.hash,
            bundleName: bundle.name || bundle.hash,
            name: bundle.name || bundle.hash,
            status: bundle.status.upload_status,
            stage: bundle.stage
          };
          const canDelete = canWrite && canDeleteFileRow(row);
          const deleting = deletingKeys.has(bundle.hash);
          return (
            <li key={bundle.hash} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-slate-100 px-3 py-2">
              <span className="min-w-0 flex-1 truncate text-sm text-slate-800" title={bundle.name || bundle.hash}>{bundle.name || bundle.hash}</span>
              <span className={`rounded-full border px-2 py-1 text-xs ${stageClass(bundle.stage)}`}>{stageLabel(bundle.stage)}</span>
              {canDelete ? (
                <button
                  type="button"
                  data-guide-anchor="delete-bundle"
                  className={`rounded px-2 py-1 text-sm text-rose-600 hover:bg-rose-50 disabled:text-slate-400 ${highlightDelete && bundle.hash === deletableBundles[0]?.hash ? 'outline outline-2 outline-cyan-500 outline-offset-2' : ''}`}
                  disabled={deleting}
                  onClick={() => onDeleteBundle(bundle)}
                >
                  {deleting ? '删除中…' : '删除压缩包'}
                </button>
              ) : <span className="text-xs text-slate-400">{!canWrite ? '只读，无法删除' : '处理中，暂不可删除'}</span>}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
