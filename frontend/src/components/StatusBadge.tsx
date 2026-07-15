interface StatusBadgeProps {
  status: string;
}

const colors: Record<string, string> = {
  ready: 'bg-emerald-500/20 text-emerald-700',
  processing: 'bg-amber-500/20 text-amber-700',
  error: 'bg-rose-500/20 text-rose-700'
};

export function StatusBadge({ status }: StatusBadgeProps) {
  const normalized = status.toLowerCase();
  const color = colors[normalized] ?? 'bg-slate-200 text-slate-700';
  return <span className={`rounded-full px-2 py-0.5 text-xs font-semibold ${color}`}>{status}</span>;
}
