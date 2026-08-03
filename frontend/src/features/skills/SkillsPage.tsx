import { useCallback, useEffect, useState } from 'react';
import { normalizeApiError, rainApi } from '../../api/client';
import type { SkillPayload, UserSkill } from '../../api/types';
import { SkillEditor } from './SkillEditor';
import { SkillReviewPanel } from './SkillReviewPanel';

export function SkillsPage() {
  const [items, setItems] = useState<UserSkill[]>([]);
  const [editing, setEditing] = useState<UserSkill | null | undefined>(undefined);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const load = useCallback(async () => { try { const data = await rainApi.fetchSkills(); setItems(data); setSelected((id) => id && data.some((item) => item.id === id) ? id : data[0]?.id ?? null); } catch (reason) { setError(normalizeApiError(reason)); } }, []);
  useEffect(() => { void load(); }, [load]);
  const selectedSkill = items.find((item) => item.id === selected) ?? null;
  const save = async (payload: SkillPayload) => { setBusy(true); setError(''); try { const value = editing ? await rainApi.updateSkill(editing.id, payload) : await rainApi.createSkill(payload); await load(); setSelected(value.id); setEditing(undefined); } catch (reason) { setError(normalizeApiError(reason)); } finally { setBusy(false); } };
  const mutate = async (operation: () => Promise<unknown>) => { setBusy(true); setError(''); try { await operation(); await load(); } catch (reason) { setError(normalizeApiError(reason)); } finally { setBusy(false); } };
  return (
    <section className="space-y-4">
      <div className="flex items-center justify-between"><div><h2 className="text-xl font-semibold">我的 Skills</h2><p className="text-sm text-slate-500">仅你本人可以查看、修改、评分和运行。</p></div><button className="rounded-lg bg-cyan-600 px-4 py-2 text-sm font-semibold text-white" onClick={() => setEditing(null)}>新建 Skill</button></div>
      {error ? <p className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700" role="alert">{error}</p> : null}
      {editing !== undefined ? <SkillEditor skill={editing} saving={busy} onSave={save} onCancel={() => setEditing(undefined)} /> : (
        <div className="grid gap-4 lg:grid-cols-[280px_1fr]">
          <div className="space-y-2">{items.length ? items.map((item) => <button key={item.id} className={`w-full rounded-lg border p-3 text-left ${selected === item.id ? 'border-cyan-400 bg-cyan-50' : 'border-slate-200'}`} onClick={() => setSelected(item.id)}><span className="block font-semibold">{item.name}</span><span className="text-xs text-slate-500">v{item.version} · {item.enabled ? '已启用' : '已停用'} · {item.review ? `${item.review.overall_score} 分` : '待评估'}</span></button>) : <p className="rounded-lg border border-dashed border-slate-300 p-6 text-center text-sm text-slate-500">暂无 Skill</p>}</div>
          {selectedSkill ? <div className="space-y-4 rounded-xl border border-slate-200 p-4"><div className="flex flex-wrap items-start justify-between gap-3"><div><h3 className="text-lg font-semibold">{selectedSkill.name}</h3><p className="text-sm text-slate-500">{selectedSkill.description || '无描述'}</p></div><div className="flex gap-2"><button className="rounded border px-3 py-1.5 text-sm" onClick={() => setEditing(selectedSkill)}>编辑</button><button className="rounded border px-3 py-1.5 text-sm" disabled={busy} onClick={() => void mutate(() => rainApi.updateSkill(selectedSkill.id, { name: selectedSkill.name, description: selectedSkill.description, skill_markdown: selectedSkill.skill_markdown, enabled: !selectedSkill.enabled }))}>{selectedSkill.enabled ? '停用' : '启用'}</button><button className="rounded border border-rose-200 px-3 py-1.5 text-sm text-rose-700" onClick={() => { if (window.confirm('确认删除该 Skill？')) void mutate(() => rainApi.deleteSkill(selectedSkill.id)); }}>删除</button></div></div><pre className="max-h-80 overflow-auto whitespace-pre-wrap rounded-lg bg-slate-950 p-4 text-sm text-slate-100">{selectedSkill.skill_markdown}</pre><button className="rounded-lg bg-cyan-600 px-4 py-2 text-sm font-semibold text-white disabled:opacity-50" disabled={busy} onClick={() => void mutate(() => rainApi.reviewSkill(selectedSkill.id))}>质量评估</button><SkillReviewPanel review={selectedSkill.review} /></div> : null}
        </div>
      )}
    </section>
  );
}
