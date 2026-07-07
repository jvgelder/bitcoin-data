import { useMemo, useState } from 'react';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { isOpenSpendableUtxo } from '../state/lightClientState';
import type { WalletLabel, WalletSpendableUtxo, WalletTransaction } from '../state/types';
import { formatSats } from '../utils/format';
import { Badge, Button, Card, EmptyState, Field, Input, PrimaryButton } from './ui';

export function LabelEditor() {
  const { labels, spendableUtxos, transactions } = useLightClientState();
  const actions = useLightClientActions();
  const [editing, setEditing] = useState<WalletLabel>();
  const [path, setPath] = useState('/');
  const [newPath, setNewPath] = useState('/');
  const tree = useMemo(() => buildTree(labels), [labels]);
  const stats = useMemo(() => buildLabelStats(labels, spendableUtxos, transactions), [labels, spendableUtxos, transactions]);

  function beginEdit(label: WalletLabel) {
    setEditing(label);
    setPath(formatPath(label.path));
  }

  function saveEdit() {
    if (!editing) return;
    actions.upsertLabel({ id: editing.id, path: splitPath(path) });
    setEditing(undefined);
  }

  function createLabel() {
    actions.createNextLabel(splitPath(newPath));
    setNewPath('/');
  }

  return (
    <div className="space-y-5">
      <Card title="Labels" subtitle="Labels are folder-like paths. The numeric label id remains stable after renaming or hierarchy changes.">
        <div className="mb-5 grid gap-3 sm:grid-cols-[1fr_auto]">
          <Field label="New label path">
            <Input value={newPath} onChange={(event) => setNewPath(event.target.value)} placeholder="/client/invoice" />
          </Field>
          <div className="flex items-end"><PrimaryButton onClick={createLabel}>Create label</PrimaryButton></div>
        </div>
        {labels.length === 0 ? <EmptyState>No labels yet.</EmptyState> : <TreeView nodes={tree} labels={labels} stats={stats} onEdit={beginEdit} />}
      </Card>

      {editing ? (
        <Card title={`Edit label ${editing.id}`} subtitle="Edit the full path, then save. The id does not change.">
          <div className="grid gap-3 sm:grid-cols-[1fr_auto_auto]">
            <Field label="Full path">
              <Input value={path} onChange={(event) => setPath(event.target.value)} placeholder="/sublabel/subsublabel" />
            </Field>
            <div className="flex items-end"><PrimaryButton onClick={saveEdit}>Save</PrimaryButton></div>
            <div className="flex items-end"><Button onClick={() => setEditing(undefined)}>Cancel</Button></div>
          </div>
        </Card>
      ) : null}
    </div>
  );
}

interface TreeNode {
  name: string;
  children: Map<string, TreeNode>;
  labelIds: number[];
}

interface LabelStats {
  currentSat: number;
  totalSat: number;
  deltaSat: number;
  currentUtxoCount: number;
}

function buildTree(labels: WalletLabel[]): TreeNode {
  const root: TreeNode = { name: '/', children: new Map(), labelIds: [] };
  for (const label of labels) {
    let node = root;
    for (const part of label.path) {
      let child = node.children.get(part);
      if (!child) {
        child = { name: part, children: new Map(), labelIds: [] };
        node.children.set(part, child);
      }
      node = child;
    }
    node.labelIds.push(label.id);
  }
  return root;
}

function TreeView({
  nodes,
  labels,
  stats,
  onEdit,
}: {
  nodes: TreeNode;
  labels: WalletLabel[];
  stats: Map<number, LabelStats>;
  onEdit(label: WalletLabel): void;
}) {
  const byId = new Map(labels.map((label) => [label.id, label]));
  return (
    <div className="rounded-xl bg-slate-950/30 px-2 py-3">
      <NodeView node={nodes} depth={0} byId={byId} stats={stats} onEdit={onEdit} />
    </div>
  );
}

function NodeView({
  node,
  depth,
  byId,
  stats,
  onEdit,
}: {
  node: TreeNode;
  depth: number;
  byId: Map<number, WalletLabel>;
  stats: Map<number, LabelStats>;
  onEdit(label: WalletLabel): void;
}) {
  const children = [...node.children.values()].sort((a, b) => a.name.localeCompare(b.name));
  return (
    <div>
      <div className="flex items-center gap-2 py-1" style={{ paddingLeft: depth * 18 }}>
        <span className="text-slate-500">📁</span>
        <span className="text-sm font-medium text-slate-200">{node.name}</span>
      </div>
      {node.labelIds.map((id) => {
        const label = byId.get(id);
        if (!label) return null;
        const stat = stats.get(id) ?? emptyStats();
        return (
          <div key={id} className="rounded-lg px-2 py-2 hover:bg-slate-900/70" style={{ paddingLeft: depth * 18 + 28 }}>
            <div className="grid gap-2 lg:grid-cols-[minmax(12rem,1fr)_auto_auto] lg:items-center">
              <div className="flex min-w-0 items-center gap-2">
                <Badge tone={id === 1 ? 'green' : 'slate'}>{id}</Badge>
                <span className="truncate text-sm text-slate-300">{id === 1 ? '/change' : formatPath(label.path)}</span>
              </div>
              <div className="grid gap-x-4 gap-y-1 text-[11px] sm:grid-cols-4 lg:flex lg:items-center lg:justify-end">
                <StatCell label="Current" value={formatSats(stat.currentSat)} />
                <StatCell label="Total" value={formatSats(stat.totalSat)} />
                <StatCell label="Delta" value={formatSignedSats(stat.deltaSat)} tone={stat.deltaSat < 0 ? 'negative' : stat.deltaSat > 0 ? 'positive' : 'neutral'} />
                <StatCell label="UTXOs" value={String(stat.currentUtxoCount)} />
              </div>
              <Button onClick={() => onEdit(label)} className="px-2.5 py-1 text-xs">Edit</Button>
            </div>
          </div>
        );
      })}
      {children.map((child) => <NodeView key={child.name} node={child} depth={depth + 1} byId={byId} stats={stats} onEdit={onEdit} />)}
    </div>
  );
}

function StatCell({ label, value, tone = 'neutral' }: { label: string; value: string; tone?: 'positive' | 'negative' | 'neutral' }) {
  const valueClass = tone === 'positive' ? 'text-emerald-300' : tone === 'negative' ? 'text-rose-300' : 'text-slate-200';
  return (
    <div className="min-w-0">
      <span className="mr-1 uppercase tracking-wide text-slate-500">{label}</span>
      <span className={`font-medium ${valueClass}`}>{value}</span>
    </div>
  );
}

function buildLabelStats(labels: WalletLabel[], utxos: WalletSpendableUtxo[], transactions: WalletTransaction[]): Map<number, LabelStats> {
  const stats = new Map<number, LabelStats>();
  for (const label of labels) stats.set(label.id, emptyStats());

  for (const utxo of utxos.filter(isOpenSpendableUtxo)) {
    const current = stats.get(utxo.labelId) ?? emptyStats();
    current.currentSat += utxo.valueSat;
    current.currentUtxoCount += 1;
    stats.set(utxo.labelId, current);
  }

  for (const tx of transactions) {
    const current = stats.get(tx.labelId) ?? emptyStats();
    current.totalSat += Math.abs(tx.amountSat);
    current.deltaSat += tx.direction === 'received' ? tx.amountSat : -tx.amountSat - (tx.feeSat ?? 0);
    stats.set(tx.labelId, current);
  }

  return stats;
}

function emptyStats(): LabelStats {
  return { currentSat: 0, totalSat: 0, deltaSat: 0, currentUtxoCount: 0 };
}

function formatSignedSats(value: number): string {
  if (value === 0) return formatSats(0);
  const prefix = value > 0 ? '+' : '-';
  return `${prefix}${formatSats(Math.abs(value))}`;
}

function splitPath(value: string): string[] {
  return value.split('/').map((part) => part.trim()).filter(Boolean).filter((part, index) => !(index === 0 && part.toLowerCase() === 'root'));
}

function formatPath(path: string[]): string {
  return `/${path.filter(Boolean).join('/')}`;
}
