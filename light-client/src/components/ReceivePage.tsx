import { useEffect, useMemo, useState } from 'react';
import { silentPaymentAddressForLabel, walletLabelIdToBip352Label } from '../keys/silentPaymentAddress';
import { QrCode } from '../qr/QrCode';
import { useLightClientState } from '../state/LightClientProvider';
import { Card, EmptyState, Field, Select } from './ui';

export function ReceivePage() {
  const state = useLightClientState();
  const receivingLabels = useMemo(() => state.labels.filter((label) => label.id !== 1), [state.labels]);
  const [labelId, setLabelId] = useState(receivingLabels[0]?.id ?? 2);
  const [address, setAddress] = useState<string>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    setAddress(undefined);
    setError(undefined);
    if (!state.walletKey) return undefined;
    silentPaymentAddressForLabel(state.walletKey, labelId, state.manifest?.network)
      .then((next) => {
        if (active) setAddress(next);
      })
      .catch((err) => {
        if (active) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      active = false;
    };
  }, [labelId, state.manifest?.network, state.walletKey]);

  if (!state.walletKey) {
    return <Card title="Receive"><EmptyState>Load a wallet before creating a receive address.</EmptyState></Card>;
  }

  return (
    <Card title="Receive" subtitle="Select a wallet label, then share the Silent Payment address. The /change label is intentionally hidden from receive because it is reserved for wallet change.">
      <div className="space-y-5">
        <Field label="Label">
          <Select value={labelId} onChange={(event) => setLabelId(Number(event.target.value))}>
            {receivingLabels.map((label) => (
              <option key={label.id} value={label.id}>/{label.path.join('/')} · label {walletLabelIdToBip352Label(label.id)}</option>
            ))}
          </Select>
        </Field>
        {error ? <p className="text-sm text-rose-200">{error}</p> : null}
        {address ? (
          <div className="grid gap-5 lg:grid-cols-[auto_1fr]">
            <QrCode value={address} title="Silent Payment address" />
            <div>
              <div className="text-xs uppercase tracking-wide text-slate-500">Silent Payment address</div>
              <div className="mt-2 break-all rounded-2xl border border-slate-800 bg-slate-950/60 p-4 font-mono text-sm text-slate-100">{address}</div>
              <p className="mt-3 text-sm text-slate-400">Address contains public scan and public spend keys. Wallet-specific private scan/spend keys are not sent to the light server.</p>
            </div>
          </div>
        ) : !error ? (
          <EmptyState>Generating address…</EmptyState>
        ) : null}
      </div>
    </Card>
  );
}
