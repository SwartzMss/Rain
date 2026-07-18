export type ConfirmDialogState = {
  message: string;
  onConfirm: () => Promise<void> | void;
  busy?: boolean;
};

type ConfirmDialogProps = {
  dialog: ConfirmDialogState;
  onCancel: () => void;
  onBusyChange: (busy: boolean) => void;
  onClose: () => void;
};

export function ConfirmDialog({ dialog, onBusyChange, onCancel, onClose }: ConfirmDialogProps) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/45 p-4 backdrop-blur-md">
      <div className="w-full max-w-sm rounded-2xl border border-white/70 bg-white/95 p-5 shadow-2xl shadow-slate-950/25">
        <p className="text-sm text-slate-700">{dialog.message}</p>
        <div className="mt-4 flex justify-end gap-3">
          <button
            type="button"
            className="rounded-lg border border-slate-300 px-4 py-2 text-sm text-slate-700"
            onClick={onCancel}
            disabled={!!dialog.busy}
          >
            取消
          </button>
          <button
            type="button"
            className="rounded-lg bg-rose-500 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60"
            disabled={!!dialog.busy}
            onClick={async () => {
              onBusyChange(true);
              try {
                await dialog.onConfirm();
              } finally {
                onClose();
              }
            }}
          >
            {dialog.busy ? '处理中...' : '确定删除'}
          </button>
        </div>
      </div>
    </div>
  );
}
