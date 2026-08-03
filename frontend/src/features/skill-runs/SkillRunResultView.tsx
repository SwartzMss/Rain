import type { SkillEvidence, SkillRunResult } from '../../api/types';

function ItemList({ title, items }: { title: string; items: unknown[] }) {
  if (!items.length) return null;
  return <section><h4 className="text-sm font-semibold text-slate-800">{title}</h4><ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-slate-600">{items.map((item, index) => <li key={index}>{typeof item === 'string' ? item : JSON.stringify(item)}</li>)}</ul></section>;
}

export function SkillRunResultView({ result, onRevealEvidence }: { result: SkillRunResult; onRevealEvidence: (evidence: SkillEvidence) => void }) {
  return (
    <div className="space-y-4 rounded-xl border border-cyan-100 bg-cyan-50/40 p-4">
      <div><h3 className="text-sm font-semibold text-slate-900">诊断结论</h3><p className="mt-1 whitespace-pre-wrap text-sm text-slate-700">{result.summary}</p></div>
      <ItemList title="观察" items={result.observations} />
      <ItemList title="推断" items={result.inferences} />
      <ItemList title="缺失上下文" items={result.missing_context} />
      {result.evidence.length ? <section><h4 className="text-sm font-semibold text-slate-800">证据</h4><div className="mt-2 space-y-2">{result.evidence.map((evidence, index) => <button key={`${evidence.file_id}:${evidence.start_line}:${index}`} className="block w-full rounded-lg border border-slate-200 bg-white p-3 text-left text-sm hover:border-cyan-400" type="button" onClick={() => onRevealEvidence(evidence)}><span className="font-mono font-semibold text-cyan-700">{evidence.path.split('/').pop()}:{evidence.start_line}-{evidence.end_line}</span><span className="mt-1 block text-slate-600">{evidence.explanation}</span><code className="mt-2 block whitespace-pre-wrap text-xs text-slate-500">{evidence.excerpt}</code></button>)}</div></section> : null}
    </div>
  );
}
