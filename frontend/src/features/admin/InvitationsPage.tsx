import { useCallback, useEffect, useState, type FormEvent } from "react";
import { normalizeApiError, rainApi } from "../../api/client";
import type { CreatedInvitationBatch, InvitationItem, InvitationStatus } from "../../api/types";

const statusLabels: Record<InvitationStatus, string> = {
  ACTIVE: "可使用",
  USED: "已使用",
  REVOKED: "已撤销",
  EXPIRED: "已过期",
};

function localDate(value: string | null): string {
  if (!value) return "永久";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export function InvitationsPage() {
  const [items, setItems] = useState<InvitationItem[]>([]);
  const [status, setStatus] = useState<"" | InvitationStatus>("");
  const [cursorHistory, setCursorHistory] = useState<Array<string | undefined>>([undefined]);
  const cursor = cursorHistory[cursorHistory.length - 1];
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [count, setCount] = useState(1);
  const [validityDays, setValidityDays] = useState("7");
  const [note, setNote] = useState("");
  const [created, setCreated] = useState<CreatedInvitationBatch | null>(null);
  const [copyMessage, setCopyMessage] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const page = await rainApi.fetchInvitations({
        ...(status ? { status } : {}),
        ...(cursor ? { cursor } : {}),
        limit: 50,
      });
      setItems(page.items);
      setNextCursor(page.next_cursor);
    } catch (cause) {
      setError(normalizeApiError(cause));
    } finally {
      setLoading(false);
    }
  }, [cursor, status]);

  useEffect(() => { void load(); }, [load]);

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSaving(true);
    setError(null);
    setCopyMessage("");
    try {
      const batch = await rainApi.createInvitations({
        count,
        validity_days: validityDays ? Number(validityDays) : null,
        note: note.trim(),
      });
      setCreated(batch);
      await load();
    } catch (cause) {
      setError(normalizeApiError(cause));
    } finally {
      setSaving(false);
    }
  };

  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyMessage("已复制到剪贴板");
    } catch {
      setCopyMessage("无法访问剪贴板，请手动选择并复制下方邀请码");
    }
  };

  const revoke = async (item: InvitationItem) => {
    if (!window.confirm("撤销后此邀请码将立即失效，且不能恢复。继续吗？")) return;
    setError(null);
    try {
      await rainApi.revokeInvitation(item.id);
      await load();
    } catch (cause) {
      const message = normalizeApiError(cause);
      await load();
      setError(message);
    }
  };

  return (
    <div className="space-y-5">
      <section className="rounded-2xl border border-slate-200/90 bg-white/95 p-5 shadow-sm sm:p-6">
        <h1 className="text-xl font-semibold text-slate-950">生成邀请码</h1>
        <p className="mt-1 text-sm leading-6 text-slate-500">每个邀请码只能注册一个账户。完整邀请码只在创建完成时展示一次，请及时保存并通过可信渠道发给受邀者。</p>
        <form className="mt-5 grid gap-4 sm:grid-cols-2 lg:grid-cols-4" onSubmit={create}>
          <label className="text-sm font-medium text-slate-700">生成数量
            <input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" type="number" min={1} max={100} required value={count} onChange={(event) => setCount(Number(event.target.value))} />
          </label>
          <label className="text-sm font-medium text-slate-700">有效期
            <select className="mt-2 w-full rounded-lg border border-slate-200 bg-white px-3 py-2.5" value={validityDays} onChange={(event) => setValidityDays(event.target.value)}>
              <option value="1">1 天</option><option value="7">7 天</option><option value="30">30 天</option><option value="">永久有效</option>
            </select>
          </label>
          <label className="text-sm font-medium text-slate-700 sm:col-span-2">备注
            <input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" maxLength={200} value={note} onChange={(event) => setNote(event.target.value)} placeholder="例如：项目组邀请" />
          </label>
          <div className="sm:col-span-2 lg:col-span-4">
            <button className="rounded-lg bg-slate-950 px-4 py-2.5 text-sm font-semibold text-white hover:bg-cyan-700 disabled:opacity-50" disabled={saving} type="submit">{saving ? "正在生成…" : "生成邀请码"}</button>
          </div>
        </form>
      </section>

      {error ? <div role="alert" className="rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">{error}</div> : null}

      <section className="rounded-2xl border border-slate-200/90 bg-white/95 p-5 shadow-sm sm:p-6">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div><h2 className="text-lg font-semibold text-slate-950">邀请码记录</h2><p className="mt-1 text-sm text-slate-500">已使用的邀请码会保留记录，撤销不会影响已注册账户。</p></div>
          <label className="text-sm text-slate-600">状态
            <select className="ml-2 rounded-lg border border-slate-200 bg-white px-3 py-2" value={status} onChange={(event) => { setStatus(event.target.value as "" | InvitationStatus); setCursorHistory([undefined]); }}>
              <option value="">全部</option><option value="ACTIVE">可使用</option><option value="USED">已使用</option><option value="REVOKED">已撤销</option><option value="EXPIRED">已过期</option>
            </select>
          </label>
        </div>
        <div className="mt-4 overflow-x-auto">
          <table className="w-full min-w-[760px] text-left text-sm">
            <thead className="border-b border-slate-200 text-xs text-slate-500"><tr><th className="py-3 pr-3">状态</th><th className="py-3 pr-3">备注</th><th className="py-3 pr-3">创建时间</th><th className="py-3 pr-3">有效期至</th><th className="py-3 pr-3">使用者</th><th className="py-3 pr-3">操作</th></tr></thead>
            <tbody className="divide-y divide-slate-100">
              {items.map((item) => <tr key={item.id}>
                <td className="py-3 pr-3">{statusLabels[item.status]}</td><td className="py-3 pr-3">{item.note || "—"}</td><td className="py-3 pr-3">{localDate(item.created_at)}</td><td className="py-3 pr-3">{localDate(item.expires_at)}</td><td className="py-3 pr-3">{item.used_by_username ? `${item.used_by_username} · ${localDate(item.used_at)}` : "—"}</td>
                <td className="py-3 pr-3">{item.status === "ACTIVE" ? <button type="button" className="font-semibold text-rose-700 hover:text-rose-900" onClick={() => void revoke(item)}>撤销</button> : "—"}</td>
              </tr>)}
              {!loading && items.length === 0 ? <tr><td className="py-8 text-center text-slate-500" colSpan={6}>暂无邀请码记录</td></tr> : null}
            </tbody>
          </table>
          {loading ? <p className="py-6 text-center text-sm text-slate-500">正在加载…</p> : null}
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button type="button" className="rounded-lg border border-slate-200 px-3 py-2 text-sm disabled:opacity-40" disabled={loading || cursorHistory.length <= 1} onClick={() => setCursorHistory((current) => current.length > 1 ? current.slice(0, -1) : current)}>上一页</button>
          <button type="button" className="rounded-lg border border-slate-200 px-3 py-2 text-sm disabled:opacity-40" disabled={loading || !nextCursor} onClick={() => nextCursor && setCursorHistory((current) => [...current, nextCursor])}>下一页</button>
        </div>
      </section>

      {created ? <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/60 p-4" role="presentation">
        <section aria-labelledby="created-invitations-title" aria-modal="true" className="max-h-[85vh] w-full max-w-2xl overflow-auto rounded-2xl bg-white p-6 shadow-2xl" role="dialog">
          <h2 id="created-invitations-title" className="text-xl font-semibold text-slate-950">邀请码已生成</h2>
          <p className="mt-2 text-sm text-amber-800">完整邀请码只展示这一次。保存并发送给受邀者后再关闭此窗口。</p>
          <div className="mt-4 space-y-2">{created.invitations.map((invitation) => <div key={invitation.id} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-200 bg-slate-50 p-3"><code className="select-all break-all text-sm font-semibold tracking-wide text-slate-900">{invitation.code}</code><button className="rounded-md border border-slate-300 bg-white px-3 py-1.5 text-sm" type="button" onClick={() => void copy(invitation.code)}>复制</button></div>)}</div>
          {copyMessage ? <p aria-live="polite" className="mt-3 text-sm text-cyan-800">{copyMessage}</p> : null}
          <div className="mt-5 flex justify-end gap-2"><button className="rounded-lg border border-slate-300 px-4 py-2 text-sm font-semibold" type="button" onClick={() => void copy(created.invitations.map((invitation) => invitation.code).join("\n"))}>复制全部</button><button className="rounded-lg bg-slate-950 px-4 py-2 text-sm font-semibold text-white" type="button" onClick={() => { setCreated(null); setCopyMessage(""); }}>我已保存，关闭</button></div>
        </section>
      </div> : null}
    </div>
  );
}
