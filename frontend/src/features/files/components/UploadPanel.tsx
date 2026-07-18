import type { FormEvent, RefObject } from 'react';
import type { UploadTaskResponse } from '../../../api/types';

type UploadPanelProps = {
  activeTask: UploadTaskResponse | null;
  currentIssueCode: string;
  fileInputRef: RefObject<HTMLInputElement>;
  onFilesSelected: (files: File[]) => void;
  uploadDisabled: boolean;
  uploadError: string | null;
  uploading: boolean;
  uploadingRef: RefObject<boolean>;
};

export function UploadPanel({
  activeTask,
  currentIssueCode,
  fileInputRef,
  onFilesSelected,
  uploadDisabled,
  uploadError,
  uploading,
  uploadingRef
}: UploadPanelProps) {
  const handleUpload = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
  };

  return (
    <form onSubmit={handleUpload} className="space-y-3 p-4">
      <h3 className="text-lg font-semibold text-slate-950">上传日志</h3>
      <input
        ref={fileInputRef}
        type="file"
        multiple
        className="hidden"
        disabled={uploadDisabled}
        onChange={(event) => {
          if (uploadDisabled || uploadingRef.current) return;
          const files = event.target.files;
          if (files?.length) {
            onFilesSelected(Array.from(files));
          }
          if (fileInputRef.current) {
            fileInputRef.current.value = '';
          }
        }}
      />
      <div
        className="flex min-h-28 items-center justify-between gap-4 rounded-xl border border-dashed border-slate-300 bg-gradient-to-br from-slate-50 to-sky-50/50 px-5 py-4 text-sm transition hover:border-sky-400 hover:shadow-[inset_0_0_0_1px_rgba(6,182,212,0.08)] aria-disabled:opacity-60"
        aria-disabled={uploadDisabled}
        onClick={() => {
          if (!uploadDisabled && !uploadingRef.current) {
            fileInputRef.current?.click();
          }
        }}
        onDragOver={(event) => {
          event.preventDefault();
          event.stopPropagation();
        }}
        onDrop={(event) => {
          event.preventDefault();
          event.stopPropagation();
          if (uploadDisabled || uploadingRef.current) return;
          if (event.dataTransfer.files.length) {
            onFilesSelected(Array.from(event.dataTransfer.files));
          }
        }}
      >
        <div className="flex min-w-0 items-center gap-4">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl bg-slate-950 text-xl text-cyan-300 shadow-md shadow-slate-900/15">
            ↑
          </div>
          <div>
            <p className="font-semibold text-slate-950">
              {!currentIssueCode
                ? '先选择或新建 Issue'
                : uploading
                  ? '处理中'
                  : activeTask
                    ? '处理中'
                    : '拖拽日志文件到这里，或点击选择文件'}
            </p>
            <p className="mt-1 text-xs text-slate-500">
              支持 .log、.txt、.zip、.tar.gz、.tgz、.gz，单个文件最大 512 MB
            </p>
          </div>
        </div>
        <button
          type="button"
          className="shrink-0 rounded-xl bg-sky-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-sky-900/15 transition hover:-translate-y-0.5 hover:bg-sky-500 disabled:opacity-60"
          disabled={uploadDisabled}
          onClick={(event) => {
            event.stopPropagation();
            if (!uploadDisabled) fileInputRef.current?.click();
          }}
        >
          {uploading || activeTask ? '处理中' : '选择文件'}
        </button>
      </div>
      {uploadError ? <p className="text-sm text-rose-600">{uploadError}</p> : null}
    </form>
  );
}
