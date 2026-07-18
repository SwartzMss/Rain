import { isArchiveNode, isBinaryNode } from '../filePresentation';
import { isExtractionFolder, type TreeNode } from '../treeModel';
import { FileIcon, FolderIcon } from './FileIcons';

type FileTreeNodeProps = {
  nodeId: string;
  depth?: number;
  treeNodes: Record<string, TreeNode>;
  expandedNodes: Set<string>;
  selectedNodeId: string | null;
  onNodeClick: (nodeId: string) => void;
};

export function FileTreeNode({
  nodeId,
  depth = 0,
  treeNodes,
  expandedNodes,
  selectedNodeId,
  onNodeClick
}: FileTreeNodeProps): JSX.Element | null {
  const node = treeNodes[nodeId];
  if (!node) return null;

  const parentNode = node.parentId ? treeNodes[node.parentId] : null;
  if (isExtractionFolder(node, parentNode)) {
    return (
      <div className="border-l border-slate-200 pl-3">
        {node.childrenIds.map((childId) => (
          <FileTreeNode
            key={childId}
            nodeId={childId}
            depth={depth}
            treeNodes={treeNodes}
            expandedNodes={expandedNodes}
            selectedNodeId={selectedNodeId}
            onNodeClick={onNodeClick}
          />
        ))}
      </div>
    );
  }

  const isExpanded = expandedNodes.has(nodeId);
  const isSelected = selectedNodeId === nodeId;
  const canExpand = node.is_dir || isArchiveNode(node);
  return (
    <div>
      <button
        type="button"
        aria-label={node.name}
        onClick={() => onNodeClick(node.id)}
        className={[
          'group flex h-9 w-full items-center gap-2 rounded-md border border-transparent px-2 text-left text-sm transition',
          isSelected ? 'border-sky-200 bg-sky-50 text-sky-700 shadow-[inset_3px_0_0_rgba(37,99,235,0.82)]' : 'text-slate-600 hover:bg-slate-100 hover:text-slate-950'
        ].join(' ')}
        style={{ paddingLeft: `${8 + depth * 16}px` }}
      >
        <span className="flex h-4 w-3 shrink-0 items-center justify-center text-slate-400">
          {canExpand ? (
            <svg aria-hidden="true" viewBox="0 0 12 12" className={`h-3 w-3 transition-transform ${isExpanded ? 'rotate-90' : ''}`} fill="currentColor">
              <path d="m4 2 4 4-4 4V2Z" />
            </svg>
          ) : null}
        </span>
        {node.is_dir ? (
          <FolderIcon open={isExpanded} className="text-amber-400" />
        ) : (
          <FileIcon name={node.name} binary={isBinaryNode(node)} />
        )}
        <span
          className="min-w-0 flex-1 truncate text-[13px] font-medium leading-4"
          title={node.name}
        >
          {node.name}
        </span>
      </button>
      {canExpand && isExpanded ? (
        <div className="ml-4 border-l border-slate-200">
          {node.childrenIds.map((childId) => (
            <FileTreeNode
              key={childId}
              nodeId={childId}
              depth={depth + 1}
              treeNodes={treeNodes}
              expandedNodes={expandedNodes}
              selectedNodeId={selectedNodeId}
              onNodeClick={onNodeClick}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}
