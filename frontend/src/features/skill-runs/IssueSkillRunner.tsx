import { useEffect, useState } from 'react';
import { normalizeApiError, rainApi } from '../../api/client';
import type { SkillEvidence, UserSkillSummary } from '../../api/types';
import { SkillRunResultView } from './SkillRunResultView';
import { useSkillRun } from './useSkillRun';

export function IssueSkillRunner({ issueCode, onRevealEvidence }: { issueCode: string; onRevealEvidence: (evidence: SkillEvidence) => void }) {
  const [skills, setSkills] = useState<UserSkillSummary[]>([]);
  const [selected, setSelected] = useState('');
  const [providerReady, setProviderReady] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState('');
  const state = useSkillRun(issueCode);
  useEffect(() => {
    Promise.all([rainApi.fetchSkills(), rainApi.fetchAiProviderStatus()]).then(([items, provider]) => {
      const enabled = items.filter((item) => item.enabled);
      setSkills(enabled); setSelected(enabled[0]?.id ?? ''); setProviderReady(provider.configured); setLoaded(true);
    }).catch((reason) => { setLoadError(normalizeApiError(reason)); setLoaded(true); });
  }, [issueCode]);
  return (
    <section className="rounded-2xl border border-slate-200 bg-white p-4 shadow-sm">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div><h2 className="font-semibold text-slate-950">Skill 诊断</h2><p className="mt-1 text-xs text-slate-500">使用你的私有 Skill 分析当前 Issue；临时结果将在 24 小时后清理。</p></div>
        <div className="flex flex-wrap items-end gap-2">
          <label className="text-xs font-medium text-slate-600">选择 Skill<select aria-label="选择 Skill" className="mt-1 block min-w-48 rounded-lg border border-slate-300 px-3 py-2 text-sm" value={selected} disabled={state.active} onChange={(event) => setSelected(event.target.value)}><option value="">请选择</option>{skills.map((skill) => <option key={skill.id} value={skill.id}>{skill.name} · v{skill.version}</option>)}</select></label>
          {state.active ? <button className="rounded-lg border border-rose-200 px-4 py-2 text-sm font-semibold text-rose-700" type="button" onClick={() => void state.cancel()}>取消诊断</button> : <button className="rounded-lg bg-cyan-600 px-4 py-2 text-sm font-semibold text-white disabled:opacity-50" type="button" disabled={!selected || !providerReady} onClick={() => void state.start(selected)}>运行 Skill</button>}
        </div>
      </div>
      {loaded && !providerReady && !loadError ? <p className="mt-3 text-sm text-amber-700">管理员尚未配置 AI Provider。</p> : null}
      {loaded && providerReady && skills.length === 0 && !loadError ? <p className="mt-3 text-sm text-amber-700">你还没有已启用的 Skill，请先到“我的 Skills”创建或启用。</p> : null}
      {state.run ? <p className="mt-3 text-xs text-slate-500">{state.run.issue_code === issueCode ? '状态' : `已有 ${state.run.issue_code} 的活动任务`}：{state.run.status} · 迭代 {state.run.iteration_count}/8 · 工具调用 {state.run.tool_call_count}/24</p> : null}
      {loadError || state.error ? <p className="mt-3 rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700" role="alert">{loadError || state.error}</p> : null}
      {state.run?.status === 'FAILED' ? <p className="mt-3 text-sm text-rose-700">诊断失败：{state.run.error_message || state.run.error_code || '未知错误'}</p> : null}
      {state.result ? <div className="mt-4"><SkillRunResultView result={state.result} onRevealEvidence={onRevealEvidence} /></div> : null}
    </section>
  );
}
