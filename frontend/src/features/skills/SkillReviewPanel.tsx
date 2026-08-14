import type { SkillReview } from '../../api/types';

export const SKILL_REVIEW_DIMENSIONS = [
  { key: 'task_scope', label: '任务范围', weight: 15 },
  { key: 'business_flow', label: '业务流程', weight: 20 },
  { key: 'signal_semantics', label: '信号语义', weight: 20 },
  { key: 'causal_relationships', label: '因果关系', weight: 20 },
  { key: 'diagnostic_usefulness', label: '诊断有效性', weight: 15 },
  { key: 'clarity', label: '表达清晰度', weight: 10 },
] as const;

export function SkillReviewPanel({ review, reviewing = false }: { review?: SkillReview | null; reviewing?: boolean }) {
  if (reviewing) {
    return <div className="rounded-xl border border-cyan-100 bg-cyan-50/50 p-4 text-sm text-cyan-800" role="status" aria-live="polite">正在评估，请稍候…</div>;
  }
  if (!review) return <p className="text-sm text-slate-500">当前版本尚未评估。</p>;
  return (
    <div className="space-y-3 rounded-xl border border-cyan-100 bg-cyan-50/50 p-4">
      <div className="flex items-center justify-between"><strong>质量评分</strong><span aria-label="总分" className="text-2xl font-semibold text-cyan-700">{review.overall_score}</span></div>
      <p className="text-xs font-semibold text-slate-500">{review.grade}</p>
      <p className="text-xs text-slate-600">当前评分评价的是 Skill 文档提供的领域知识质量，不代表某次 Skill Run 的实际诊断准确率。</p>
      <dl className="grid gap-2 text-sm sm:grid-cols-2">
        {SKILL_REVIEW_DIMENSIONS.map(({ key, label, weight }) => <div key={key} className="flex items-center justify-between rounded bg-white px-3 py-2"><dt><span>{label}</span><span className="ml-2 text-xs font-normal text-slate-400">{weight}%</span></dt><dd className="font-semibold">{String(review.dimensions[key] ?? '—')}</dd></div>)}
      </dl>
      {review.warnings.length ? <div><h4 className="text-sm font-semibold text-amber-800">警告</h4><ul className="list-disc pl-5 text-sm text-amber-800">{review.warnings.map((item) => <li key={item}>{item}</li>)}</ul></div> : null}
      {review.suggestions.length ? <div><h4 className="text-sm font-semibold text-slate-700">建议</h4><ul className="list-disc pl-5 text-sm text-slate-600">{review.suggestions.map((item) => <li key={item}>{item}</li>)}</ul></div> : null}
    </div>
  );
}
