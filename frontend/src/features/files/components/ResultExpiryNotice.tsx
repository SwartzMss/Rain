type Props = {
  onReplay?: () => void;
  replaying?: boolean;
  error?: string | null;
};

export function ResultExpiryNotice({ onReplay, replaying, error }: Props) {
  return (
    <div role="status" className="border-b border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
      <p>搜索结果已过期或被删除，已加载的内容仍保留。</p>
      {onReplay ? <>
        <p className="mt-1 text-xs">重新搜索会生成新结果，原始数据可能已变化。</p>
        <button type="button" disabled={replaying} onClick={onReplay}
          className="mt-2 rounded border border-amber-400 px-3 py-1 disabled:opacity-50">
          {replaying ? '正在重新搜索…' : '重新搜索'}
        </button>
      </> : <p className="mt-1 text-xs">请返回原始文件或 Issue 重新搜索。</p>}
      {error ? <p role="alert" className="mt-2 text-rose-700">{error}</p> : null}
    </div>
  );
}
