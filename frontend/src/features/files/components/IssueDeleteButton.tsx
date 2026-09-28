type IssueDeleteButtonProps = {
  issueCode: string;
  canWrite: boolean;
  blocked: boolean;
  deleting: boolean;
  onDelete: () => void;
};

export function IssueDeleteButton({
  issueCode,
  canWrite,
  blocked,
  deleting,
  onDelete
}: IssueDeleteButtonProps) {
  if (!canWrite || !issueCode) return null;

  const disabled = blocked || deleting;
  return (
    <div className="flex flex-col items-end gap-1">
      <button
        type="button"
        className="rounded-lg border border-rose-500/60 px-4 py-2 text-sm font-semibold text-rose-600 transition hover:bg-rose-500/10 disabled:cursor-not-allowed disabled:opacity-60"
        disabled={disabled}
        onClick={onDelete}
      >
        删除 Issue
      </button>
    </div>
  );
}
