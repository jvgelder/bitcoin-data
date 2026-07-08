import { useMemo } from 'react';
import { useLightClientState } from '../state/LightClientProvider';
import { deriveOpenUtxoBalance } from '../state/lightClientState';
import type { WalletLabel } from '../state/types';
import { formatBtcFromSats, formatDateTime, formatFiatFromSats, formatSats, shortHash } from '../utils/format';
import { Badge, Button, Card, EmptyState, PrimaryButton } from './ui';

export function Dashboard({ onOpenTransaction, onSend, onReceive }: { onOpenTransaction(id: string): void; onSend(): void; onReceive(): void }) {
  const state = useLightClientState();
  const labelsById = useMemo(() => new Map(state.labels.map((label) => [label.id, label])), [state.labels]);
  const balanceSat = useMemo(() => deriveOpenUtxoBalance(state.spendableUtxos), [state.spendableUtxos]);
  const sortedTransactions = [...state.transactions].sort((a, b) => new Date(b.dateTime).getTime() - new Date(a.dateTime).getTime());

  return (
    <div className="space-y-5">
      <Card className="bg-slate-900/90">
        <div className="flex flex-col gap-6 sm:flex-row sm:items-end sm:justify-between">
          <div>
            <p className="text-sm uppercase tracking-[0.2em] text-slate-500">Wallet balance</p>
            <div className="mt-2 text-4xl font-semibold tracking-tight text-white">{formatBtcFromSats(balanceSat)}</div>
            <div className="mt-2 text-sm text-slate-400">
              {formatSats(balanceSat)} · {formatFiatFromSats(balanceSat, state.wallet.fiatRatePerBtc, state.wallet.selectedFiat)}
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3 sm:w-72">
            <PrimaryButton disabled={!state.walletKey} onClick={onSend}>Send</PrimaryButton>
            <Button disabled={!state.walletKey} onClick={onReceive}>Receive</Button>
          </div>
        </div>
      </Card>

      {state.demo.enabled ? (
        <Card className="border-amber-800 bg-amber-950/20">
          <div className="text-sm text-amber-100">Demo mode is enabled. Transactions and spendable outputs are examples for testing the UI, PSBT flow, and RBF screens.</div>
        </Card>
      ) : null}

      <Card title="Recent transactions" subtitle="Rows open a detailed transaction view. Real history will come from confirmed wallet scans/imports.">
        {sortedTransactions.length === 0 ? (
          <EmptyState>No wallet transactions yet. Import existing history or sync after scanner integration.</EmptyState>
        ) : (
          <div className="divide-y divide-slate-800 overflow-hidden rounded-xl border border-slate-800">
            {sortedTransactions.map((tx) => (
              <button
                key={tx.id}
                type="button"
                onClick={() => onOpenTransaction(tx.id)}
                className="grid w-full gap-2 bg-slate-950/40 px-4 py-3 text-left transition hover:bg-slate-900 sm:grid-cols-[1fr_auto]"
              >
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium text-slate-100">{formatDateTime(tx.dateTime)}</span>
                    <Badge tone={tx.direction === 'received' ? 'green' : 'yellow'}>{tx.direction}</Badge>
                    <span className="text-xs text-slate-500">{labelName(labelsById.get(tx.labelId))}</span>
                  </div>
                  <div className="mt-1 font-mono text-xs text-slate-500">{shortHash(tx.txid)}</div>
                </div>
                <div className="sm:text-right">
                  <div className={tx.direction === 'received' ? 'font-semibold text-emerald-200' : 'font-semibold text-amber-200'}>
                    {tx.direction === 'received' ? '+' : '-'}{formatSats(tx.amountSat)}
                  </div>
                  <div className="mt-1 text-xs text-slate-500">
                    {formatFiatFromSats(tx.amountSat, state.wallet.fiatRatePerBtc, state.wallet.selectedFiat)}
                  </div>
                </div>
              </button>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}

function labelName(label?: WalletLabel): string {
  if (!label) return 'unlabeled';
  if (label.id === 1) return 'change';
  return `/${label.path.join('/')}`;
}
