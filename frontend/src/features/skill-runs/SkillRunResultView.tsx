import type { SkillEvidence, SkillInference, SkillObservation, SkillRunResult } from '../../api/types';

function ObservationList({ items }: { items: SkillObservation[] }) {
  if (!items.length) return null;
  return <section><h4 className="text-sm font-semibold text-slate-800">观察</h4><ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-slate-600">{items.map((item, index) => <li key={index}>{item.text} <span className="text-xs text-slate-400">[{item.evidence_ids.join(', ')}]</span></li>)}</ul></section>;
}

function InferenceList({ items }: { items: SkillInference[] }) {
  if (!items.length) return null;
  return <section><h4 className="text-sm font-semibold text-slate-800">推断</h4><ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-slate-600">{items.map((item, index) => <li key={index}>{item.text} <span className="text-xs text-slate-400">{item.confidence} · [{item.evidence_ids.join(', ')}]</span></li>)}</ul></section>;
}

function MissingContextList({ items }: { items: string[] }) {
  if (!items.length) return null;
  return <section><h4 className="text-sm font-semibold text-slate-800">缺失上下文</h4><ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-slate-600">{items.map((item, index) => <li key={index}>{item}</li>)}</ul></section>;
}

export function SkillRunResultView({ result, onRevealEvidence }: { result: SkillRunResult; onRevealEvidence: (evidence: SkillEvidence) => void }) {
  return (
    <div className="space-y-4 rounded-xl border border-cyan-100 bg-cyan-50/40 p-4">
      <div><h3 className="text-sm font-semibold text-slate-900">诊断结论</h3><p className="mt-1 whitespace-pre-wrap text-sm text-slate-700">{result.summary}</p></div>
      <ObservationList items={result.observations} />
      <InferenceList items={result.inferences} />
      <MissingContextList items={result.missing_context} />
      {result.evidence.length ? <section><h4 className="text-sm font-semibold text-slate-800">证据</h4><div className="mt-2 space-y-2">{result.evidence.map((evidence) => <button key={evidence.id} className="block w-full rounded-lg border border-slate-200 bg-white p-3 text-left text-sm hover:border-cyan-400" type="button" onClick={() => onRevealEvidence(evidence)}><span className="font-mono font-semibold text-cyan-700">[{evidence.id}] {evidence.path.split('/').pop()}:{evidence.start_line}-{evidence.end_line}</span><span className="mt-1 block text-slate-600">{evidence.explanation}</span><code className="mt-2 block whitespace-pre-wrap text-xs text-slate-500">{evidence.excerpt}</code></button>)}</div></section> : null}
    </div>
  );
}
