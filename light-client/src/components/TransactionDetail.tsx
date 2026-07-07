import { useMemo, useState } from 'react';
import type { WalletTransaction } from '../state/types';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { enabledProviders } from '../state/lightClientState';
import { broadcastTransactionWithFallback, type BlockProviderConfig } from '../api/blockProviderApi';
import { createRbfReplacementFromRawTx } from '../payments/psbt';
import { formatDateTime, formatFiatFromSats, formatSats, shortHash } from '../utils/format';
import { Badge, Button, Card, EmptyState, Field, Input, PrimaryButton } from './ui';

export function TransactionDetail({ transaction, onBack }: { transaction?: WalletTransaction; onBack(): void }) {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const [extraFeeSat, setExtraFeeSat] = useState('500');
  const [rbfMessage, setRbfMessage] = useState<{ tone: 'error' | 'success' | 'info'; text: string }>();
  const label = transaction ? state.labels.find((entry) => entry.id === transaction.labelId) : undefined;
  const blockProviders = useMemo<BlockProviderConfig[]>(() => enabledProviders(state.providers.blockProviders, state.providers.activeBlockProviderId).map((provider) => ({
    name: provider.name,
    url: provider.url,
    apiKey: provider.apiKey,
    apiKeyPlacement: provider.apiKeyPlacement,
    apiKeyName: provider.apiKeyName,
  })), [state.providers]);

  if (!transaction) {
    return (
      <Card title="Transaction">
        <EmptyState>Transaction not found.</EmptyState>
        <div className="mt-5"><Button onClick={onBack}>Back</Button></div>
      </Card>
    );
  }

  const fiatValue = formatFiatFromSats(transaction.amountSat, state.wallet.fiatRatePerBtc, state.wallet.selectedFiat);
  const canRbf = transaction.direction === 'sent' && (transaction.confirmations ?? 0) === 0;

  async function rebroadcastRbf() {
    if (!transaction) return;
    try {
      if (!transaction.rawTxHex || transaction.rbfChangeOutputIndex == null) {
        throw new Error('This transaction does not have the raw transaction/change-output metadata needed to build an RBF replacement. Recreate a replacement PSBT from the original wallet inputs instead.');
      }
      const replacement = createRbfReplacementFromRawTx(transaction.rawTxHex, transaction.rbfChangeOutputIndex, Number(extraFeeSat));
      if (transaction.demo) {
        actions.addWalletTransaction({
          ...transaction,
          id: replacement.txid,
          txid: replacement.txid,
          feeSat: (transaction.feeSat ?? 0) + replacement.feeDeltaSat,
          rawTxHex: replacement.rawTxHex,
          dateTime: new Date().toISOString(),
          note: `Demo RBF replacement for ${shortHash(transaction.txid)}. Fee increased by ${formatSats(replacement.feeDeltaSat)}. No network broadcast was attempted.`,
        });
        setRbfMessage({ tone: 'success', text: `Demo replacement created locally: ${replacement.txid}` });
        return;
      }
      setRbfMessage({ tone: 'info', text: `Replacement prepared: ${replacement.txid}. Broadcasting…` });
      const result = await broadcastTransactionWithFallback(blockProviders, replacement.rawTxHex);
      actions.addWalletTransaction({
        ...transaction,
        id: result.txid,
        txid: result.txid,
        feeSat: (transaction.feeSat ?? 0) + replacement.feeDeltaSat,
        rawTxHex: replacement.rawTxHex,
        dateTime: new Date().toISOString(),
        note: `RBF replacement for ${shortHash(transaction.txid)}. Fee increased by ${formatSats(replacement.feeDeltaSat)}.`,
      });
      setRbfMessage({ tone: 'success', text: `Broadcast replacement through ${result.providerName}: ${result.txid}` });
    } catch (error) {
      setRbfMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  return (
    <Card title="Transaction detail" subtitle="Wallet state still requires full-block confirmation before final scanner integration.">
      <div className="mb-5 flex flex-wrap items-center gap-2">
        <Badge tone={transaction.direction === 'received' ? 'green' : 'yellow'}>{transaction.direction}</Badge>
        {transaction.confirmations != null ? <Badge>{transaction.confirmations} confirmations</Badge> : null}
        {transaction.demo ? <Badge tone="yellow">demo</Badge> : null}
        {canRbf ? <Badge tone="blue">RBF candidate</Badge> : null}
      </div>
      <dl className="grid gap-4 text-sm sm:grid-cols-2">
        <Info label="Amount" value={`${formatSats(transaction.amountSat)}${transaction.feeSat != null ? ` (${formatSats(transaction.feeSat)} fee)` : ''}`} />
        <Info label={`Amount in ${state.wallet.selectedFiat}`} value={fiatValue} />
        <Info label="Date" value={formatDateTime(transaction.dateTime)} />
        <Info label="Label" value={label?.id === 1 ? 'change' : label ? `/${label.path.join('/')}` : 'unlabeled'} />
        <Info label="Transaction ID" value={transaction.txid} mono />
        <Info label="Short ID" value={shortHash(transaction.txid)} mono />
      </dl>
      {transaction.note ? <p className="mt-5 rounded-xl border border-slate-800 bg-slate-950/50 p-4 text-sm text-slate-300">{transaction.note}</p> : null}

      {canRbf ? (
        <section className="mt-5 rounded-2xl border border-slate-800 bg-slate-950/40 p-4">
          <h3 className="font-semibold text-slate-100">Increase fee with RBF</h3>
          <p className="mt-1 text-sm text-slate-400">
            Add a few sats to the fee and rebroadcast a replacement. Demo transactions can build a raw replacement preview; real wallet RBF requires the original PSBT/signing path so the replacement can be re-signed.
          </p>
          <div className="mt-4 grid gap-3 sm:grid-cols-[1fr_auto]">
            <Field label="Additional fee sats">
              <Input type="number" min={1} value={extraFeeSat} onChange={(event) => setExtraFeeSat(event.target.value)} />
            </Field>
            <div className="flex items-end"><PrimaryButton onClick={() => void rebroadcastRbf()}>Rebroadcast replacement</PrimaryButton></div>
          </div>
          {rbfMessage ? <p className={messageClass(rbfMessage.tone)}>{rbfMessage.text}</p> : null}
        </section>
      ) : null}

      <div className="mt-5"><Button onClick={onBack}>Back to wallet</Button></div>
    </Card>
  );
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-950/40 p-4">
      <dt className="text-xs uppercase tracking-wide text-slate-500">{label}</dt>
      <dd className={`mt-2 break-all text-slate-100 ${mono ? 'font-mono text-xs' : 'text-sm'}`}>{value}</dd>
    </div>
  );
}

function messageClass(tone: 'success' | 'error' | 'info'): string {
  if (tone === 'success') return 'mt-4 rounded-xl border border-emerald-800 bg-emerald-950/40 p-3 text-sm text-emerald-100';
  if (tone === 'error') return 'mt-4 rounded-xl border border-rose-800 bg-rose-950/40 p-3 text-sm text-rose-100';
  return 'mt-4 rounded-xl border border-sky-800 bg-sky-950/40 p-3 text-sm text-sky-100';
}
