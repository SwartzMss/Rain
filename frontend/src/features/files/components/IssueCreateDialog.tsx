type IssueCreateDialogProps = {
  creating: boolean;
  error: string | null;
  issueCode: string;
  onChangeIssueCode: (value: string) => void;
  onClose: () => void;
  onSubmit: () => void;
};

export function IssueCreateDialog({
  creating,
  error,
  issueCode,
  onChangeIssueCode,
  onClose,
  onSubmit
}: IssueCreateDialogProps) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 p-4 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <form
        className="w-full max-w-md rounded-lg border border-slate-200 bg-white p-5 shadow-2xl"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
        onKeyDown={(event) => {
          if (event.key === 'Escape') onClose();
        }}
      >
        <h3 className="text-lg font-semibold text-slate-950">新建 Issue</h3>
        <label className="mt-4 block text-sm text-slate-600">
          Issue ID
          <input
            className="mt-1 w-full rounded-lg border border-slate-300 bg-white px-4 py-2 text-slate-950 outline-none focus:border-sky-500"
            value={issueCode}
            onChange={(event) => onChangeIssueCode(event.target.value)}
            placeholder="例如 CN014"
          />
        </label>
        {error ? <p className="mt-3 text-sm text-rose-600">{error}</p> : null}
        <div className="mt-5 flex justify-end gap-3">
          <button
            type="button"
            className="rounded-lg border border-slate-300 px-4 py-2 text-sm text-slate-700"
            onClick={onClose}
            disabled={creating}
          >
            取消
          </button>
          <button
            type="submit"
            className="rounded-lg bg-sky-600 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60"
            disabled={creating}
          >
            {creating ? '创建中...' : '创建'}
          </button>
        </div>
      </form>
    </div>
  );
}
