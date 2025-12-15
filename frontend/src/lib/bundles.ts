export interface BundleInfo {
  hash: string;
  name: string;
  issue?: string;
}

export function formatBundleLabel(bundle: BundleInfo): string {
  const suffix = bundle.hash.length > 8 ? bundle.hash.slice(-8) : bundle.hash;
  const issueLabel = bundle.issue ? `${bundle.issue} · ` : '';
  return `${issueLabel}${bundle.name} (${suffix})`;
}
