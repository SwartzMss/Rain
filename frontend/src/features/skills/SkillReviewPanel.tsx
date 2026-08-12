import type { SkillReview } from '../../api/types';

export function SkillReviewPanel({ review, reviewing = false }: { review?: SkillReview | null; reviewing?: boolean }) {
  if (reviewing) {
    return <div className="rounded-xl border border-cyan-100 bg-cyan-50/50 p-4 text-sm text-cyan-800" role="status" aria-live="polite">正在评估，请稍候…</div>;
  }
  if (!review) return <p className="text-sm text-slate-500">当前版本尚未评估。</p>;
  return (
    <div className="space-y-3 rounded-xl border border-cyan-100 bg-cyan-50/50 p-4">
      <div className="flex items-center justify-between"><strong>质量评分</strong><span className="text-2xl font-semibold text-cyan-700">{review.overall_score}</span></div>
      <p className="text-xs font-semibold text-slate-500">{review.grade}</p>
      <dl className="grid gap-2 text-sm sm:grid-cols-2">
        {Object.entries(review.dimensions).map(([name, value]) => <div key={name} className="flex justify-between rounded bg-white px-3 py-2"><dt>{name}</dt><dd className="font-semibold">{String(value)}</dd></div>)}
      </dl>
      {review.warnings.length ? <div><h4 className="text-sm font-semibold text-amber-800">警告</h4><ul className="list-disc pl-5 text-sm text-amber-800">{review.warnings.map((item) => <li key={item}>{item}</li>)}</ul></div> : null}
      {review.suggestions.length ? <div><h4 className="text-sm font-semibold text-slate-700">建议</h4><ul className="list-disc pl-5 text-sm text-slate-600">{review.suggestions.map((item) => <li key={item}>{item}</li>)}</ul></div> : null}
    </div>
  );
}
