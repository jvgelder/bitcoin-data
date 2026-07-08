import { useState } from 'react';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { formatDateTime, formatInteger } from '../utils/format';
import { Badge, Button, Card, DangerButton, Field, Input, PrimaryButton } from './ui';

export function WalletPanel() {
  const { walletOutputs } = useLightClientState();
  const actions = useLightClientActions();
  const [uid, setUid] = useState('');

  function addUid() {
    const parsed = Number(uid);
    if (!Number.isSafeInteger(parsed) || parsed < 0) return;
    actions.addTrackedUid(parsed);
    setUid('');
  }

  return (
    <Card title="Tracked wallet UIDs" subtitle="Spent-ID matches are treated as signals until full Bitcoin block confirmation.">
      <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
        <Field label="UID to watch">
          <Input
            type="number"
            min={0}
            value={uid}
            onChange={(event) => setUid(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') addUid();
            }}
            placeholder="e.g. 123456789"
          />
        </Field>
        <div className="flex items-end">
          <PrimaryButton onClick={addUid} className="w-full sm:w-auto">
            Track UID
          </PrimaryButton>
        </div>
      </div>

      <div className="mt-5 space-y-3">
        {walletOutputs.length === 0 ? (
          <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-sm text-slate-500">
            Add confirmed wallet UIDs here to detect spent-ID signals during sync.
          </div>
        ) : (
          walletOutputs.map((output) => (
            <div key={output.uid} className="rounded-xl border border-slate-800 bg-slate-950/50 p-3">
              <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                <div>
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-sm font-medium text-slate-100">UID {formatInteger(output.uid)}</span>
                    <StatusBadge status={output.status} />
                  </div>
                  <p className="mt-1 text-xs text-slate-500">
                    Added {formatDateTime(output.addedAt)}
                    {output.spendSignalHeight != null ? ` · last spend signal at ${output.spendSignalHeight}` : ''}
                  </p>
                </div>
                <div className="flex gap-2">
                  {output.status === 'spend-signal' && (
                    <Button onClick={() => actions.markSpendConfirmed(output.uid)}>Mark confirmed</Button>
                  )}
                  <DangerButton onClick={() => actions.removeTrackedUid(output.uid)}>Remove</DangerButton>
                </div>
              </div>
            </div>
          ))
        )}
      </div>
    </Card>
  );
}

function StatusBadge({ status }: { status: string }) {
  if (status === 'confirmed-spent') return <Badge tone="green">confirmed spent</Badge>;
  if (status === 'spend-signal') return <Badge tone="yellow">spend signal</Badge>;
  return <Badge tone="slate">tracked</Badge>;
}
