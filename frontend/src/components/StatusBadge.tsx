interface StatusBadgeProps {
  status: string;
}

const colors: Record<string, string> = {
  ready: 'bg-emerald-500/20 text-emerald-300',
  processing: 'bg-amber-500/20 text-amber-200',
  error: 'bg-rose-500/20 text-rose-200'
};

export function StatusBadge({ status }: StatusBadgeProps) {
  const normalized = status.toLowerCase();
  const color = colors[normalized] ?? 'bg-slate-700 text-slate-200';
  return <span className={`rounded-full px-2 py-0.5 text-xs font-semibold ${color}`}>{status}</span>;
}
