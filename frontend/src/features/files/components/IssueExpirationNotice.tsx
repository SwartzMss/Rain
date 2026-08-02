import type { IssueInactivityExpiry } from '../../../api/types';
import { NOTICE_WINDOW_MS } from '../issueExpiration';

const ONE_DAY_MS = 24 * 60 * 60 * 1000;
const ONE_HOUR_MS = 60 * 60 * 1000;

function parseServerDate(value: string): Date {
  return new Date(value);
}

function formatLocalDate(date: Date): string {
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false
  }).format(date);
}

export function IssueExpirationNotice({
  canWrite,
  expiry
}: {
  canWrite: boolean;
  expiry: IssueInactivityExpiry | null;
}) {
  if (!canWrite || !expiry) return null;

  const expiresAt = parseServerDate(expiry.expires_at);
  const remainingMs = expiresAt.getTime() - Date.now();
  const validDate = Number.isFinite(expiresAt.getTime());
  if (validDate && remainingMs > NOTICE_WINDOW_MS && !expiry.renewed_from_expiring) return null;

  let title = '该 Issue 已启用自动过期';
  let urgent = false;
  let renewed = false;
  if (expiry.renewed_from_expiring) {
    title = '本次访问已将自动过期时间顺延';
    renewed = true;
  } else if (validDate && remainingMs <= 0) {
    title = '该 Issue 已进入自动清理条件';
    urgent = true;
  } else if (validDate && remainingMs <= ONE_DAY_MS) {
    title = `距离自动过期还有 ${Math.max(1, Math.ceil(remainingMs / ONE_HOUR_MS))} 小时`;
    urgent = true;
  } else if (validDate) {
    title = `距离自动过期还有 ${Math.ceil(remainingMs / ONE_DAY_MS)} 天`;
  }

  return (
    <div
      className={`mt-3 rounded-lg border px-3 py-2 text-sm ${
        renewed
          ? 'border-cyan-200 bg-cyan-50 text-cyan-800'
          : urgent
          ? 'border-rose-200 bg-rose-50 text-rose-800'
          : 'border-amber-200 bg-amber-50 text-amber-800'
      }`}
      role="status"
    >
      <p className="font-semibold">{title}</p>
      {validDate ? (
        <p className="mt-1 text-xs opacity-80">
          预计时间：{formatLocalDate(expiresAt)}　访问或操作该 Issue 后会自动顺延
        </p>
      ) : null}
      <p className="mt-1 text-xs opacity-80">
        自动清理将删除整个 Issue 及其全部 Bundle、文件和日志内容。
      </p>
    </div>
  );
}
