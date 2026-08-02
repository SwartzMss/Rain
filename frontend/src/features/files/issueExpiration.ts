import type { IssueInactivityExpiry } from '../../api/types';

export const NOTICE_WINDOW_MS = 72 * 60 * 60 * 1000;

export function visibleInactivityExpiry(
  expiry: IssueInactivityExpiry | null,
  now = Date.now()
): IssueInactivityExpiry | null {
  if (!expiry) return null;
  const expiresAt = new Date(expiry.expires_at).getTime();
  if (!Number.isFinite(expiresAt)) return expiry;
  return expiresAt - now > NOTICE_WINDOW_MS ? null : expiry;
}
