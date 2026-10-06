import type { SearchQueryPlan, ViewerTab } from './viewerTabs';

const SHARE_VERSION = '1';

export type SharedTabDescriptor =
  | {
      kind: 'file';
      issueCode: string;
      bundleHash: string;
      fileId: string;
    }
  | {
      kind: 'search';
      issueCode: string;
      plan: SearchQueryPlan;
    };

function encode(value: string): string {
  return encodeURIComponent(value);
}

function splitNodeId(nodeId: string): { bundleHash: string; fileId: string } | null {
  const separator = nodeId.indexOf(':');
  if (separator <= 0 || separator === nodeId.length - 1) return null;
  return {
    bundleHash: nodeId.slice(0, separator),
    fileId: nodeId.slice(separator + 1)
  };
}

export function buildTabShareUrl(tab: ViewerTab, issueCode: string, origin: string): string | null {
  const params = new URLSearchParams();
  params.set('share', '1');
  params.set('v', SHARE_VERSION);

  if (tab.kind === 'file') {
    const source = splitNodeId(tab.nodeId);
    if (!source || !issueCode) return null;
    params.set('view', 'file');
    params.set('bundle', source.bundleHash);
    params.set('file', source.fileId);
    return `${origin}/issue/${encode(issueCode)}/bundle/${encode(source.bundleHash)}?${params.toString()}`;
  }

  if (tab.kind !== 'search' || !tab.queryPlan || tab.queryPlan.expressions.length === 0) return null;
  const { root, expressions } = tab.queryPlan;
  if (root.kind === 'issue') {
    params.set('scope', 'issue');
    params.set('view', 'search');
    params.set('issue', root.issueCode);
  } else {
    params.set('scope', 'file');
    params.set('view', 'search');
    params.set('bundle', root.bundleHash);
    params.set('file', root.fileId);
  }
  expressions.forEach((expression) => params.append('q', expression));
  const path = root.kind === 'issue'
    ? `/issue/${encode(issueCode || root.issueCode)}`
    : `/issue/${encode(issueCode)}/bundle/${encode(root.bundleHash)}`;
  return `${origin}${path}?${params.toString()}`;
}

export function parseSharedTabSearch(search: string, routeIssueCode: string): SharedTabDescriptor | null {
  const params = new URLSearchParams(search);
  if (params.get('share') !== '1' || params.get('v') !== SHARE_VERSION) return null;
  const issueCode = routeIssueCode || params.get('issue') || '';
  const view = params.get('view');
  if (!issueCode || view === null) return null;

  if (view === 'file') {
    const bundleHash = params.get('bundle');
    const fileId = params.get('file');
    if (!bundleHash || !fileId) return null;
    return { kind: 'file', issueCode, bundleHash, fileId };
  }

  if (view !== 'search') return null;
  const expressions = params.getAll('q').filter((expression) => expression.trim().length > 0);
  if (expressions.length === 0) return null;
  if (params.get('scope') === 'issue') {
    return {
      kind: 'search',
      issueCode,
      plan: { root: { kind: 'issue', issueCode }, expressions }
    };
  }
  if (params.get('scope') === 'file') {
    const bundleHash = params.get('bundle');
    const fileId = params.get('file');
    if (!bundleHash || !fileId) return null;
    return {
      kind: 'search',
      issueCode,
      plan: { root: { kind: 'file', bundleHash, fileId }, expressions }
    };
  }
  return null;
}
