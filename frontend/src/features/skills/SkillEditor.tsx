import { useEffect, useState, type FormEvent } from 'react';
import type { SkillPayload, UserSkill } from '../../api/types';
import { DEFAULT_SKILL_MARKDOWN, REQUIRED_SKILL_SECTIONS, SKILL_SCHEMA_VERSION, UNRECOGNIZED_SKILL_SCHEMA_LABEL } from './skillSchema';

export function SkillEditor({ skill, saving, onSave, onCancel }: { skill?: UserSkill | null; saving: boolean; onSave: (payload: SkillPayload) => Promise<void>; onCancel: () => void }) {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [markdown, setMarkdown] = useState('');
  const [enabled, setEnabled] = useState(true);
  useEffect(() => {
    setName(skill?.name ?? ''); setDescription(skill?.description ?? '');
    setMarkdown(skill?.skill_markdown ?? DEFAULT_SKILL_MARKDOWN); setEnabled(skill?.enabled ?? true);
  }, [skill]);
  const dirty = name !== (skill?.name ?? '')
    || description !== (skill?.description ?? '')
    || markdown !== (skill?.skill_markdown ?? DEFAULT_SKILL_MARKDOWN)
    || enabled !== (skill?.enabled ?? true);
  const cancel = () => {
    if (!dirty || window.confirm('放弃尚未保存的修改？')) onCancel();
  };
  const submit = (event: FormEvent) => { event.preventDefault(); void onSave({ name: name.trim(), description: description.trim() || null, skill_markdown: markdown, enabled }); };
  return (
    <form className="space-y-4 rounded-xl border border-slate-200 bg-slate-50 p-4" onSubmit={submit}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-semibold">{skill ? '编辑 Skill' : '新建 Skill'}</h3>
        <span className="rounded-full bg-cyan-100 px-2.5 py-1 text-xs font-semibold text-cyan-800">
          schema_version: {skill ? skill.schema_version ?? UNRECOGNIZED_SKILL_SCHEMA_LABEL : SKILL_SCHEMA_VERSION}
        </span>
      </div>
      <label className="block text-sm font-medium">名称<input className="mt-1.5 w-full rounded-lg border border-slate-300 bg-white px-3 py-2" maxLength={100} required value={name} onChange={(e) => setName(e.target.value)} /></label>
      <label className="block text-sm font-medium">描述<textarea className="mt-1.5 w-full rounded-lg border border-slate-300 bg-white px-3 py-2" maxLength={1000} rows={2} value={description} onChange={(e) => setDescription(e.target.value)} /></label>
      <div className="rounded-lg border border-cyan-200 bg-cyan-50 px-3 py-2 text-xs text-slate-700">
        <p className="font-semibold text-cyan-900">v1 必填一级章节</p>
        <p className="mt-1 text-slate-600">标题必须严格匹配，每个章节的正文都不能为空。可以继续添加其他自定义章节。</p>
        <ul aria-label="标准中文必填章节" className="mt-2 flex flex-wrap gap-1.5">
          {REQUIRED_SKILL_SECTIONS.map((section) => <li className="rounded bg-white px-2 py-1 font-mono text-slate-700" key={section}># {section}</li>)}
        </ul>
      </div>
      <div>
        <label className="block text-sm font-medium" htmlFor="skill-markdown">SKILL.md</label>
        <textarea aria-describedby="skill-markdown-size" className="mt-1.5 min-h-72 w-full rounded-lg border border-slate-300 bg-white px-3 py-2 font-mono text-sm" id="skill-markdown" required value={markdown} onChange={(e) => setMarkdown(e.target.value)} />
        <span className="mt-1 block text-xs text-slate-500" id="skill-markdown-size">{new TextEncoder().encode(markdown).length} / 65536 bytes</span>
      </div>
      <label className="flex items-center gap-2 text-sm"><input checked={enabled} type="checkbox" onChange={(e) => setEnabled(e.target.checked)} />启用</label>
      <div className="flex gap-2"><button className="rounded-lg bg-slate-950 px-4 py-2 text-sm font-semibold text-white disabled:opacity-50" disabled={saving} type="submit">保存</button><button className="rounded-lg border border-slate-300 px-4 py-2 text-sm" type="button" onClick={cancel}>取消</button></div>
    </form>
  );
}
