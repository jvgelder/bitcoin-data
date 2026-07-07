import { useState } from 'react';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { Card, DangerButton, Field, Input, PrimaryButton } from './ui';

export function SettingsPanel() {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const busy = state.status === 'syncing' || state.status === 'connecting';
  const [height, setHeight] = useState(String(state.settings.rescanHeight));

  function prepareRescan() {
    const parsed = Number(height);
    if (!Number.isSafeInteger(parsed) || parsed < 0) return;
    actions.rescanFromHeight(parsed);
  }

  return (
    <Card title="Settings" subtitle="Control local sync behavior without changing the light-server profile.">
      <div className="grid gap-4 sm:grid-cols-[1fr_auto]">
        <Field label="Rescan from height" hint="Resets local block summaries and spend signals, then makes this the next bounded-sync start height.">
          <Input
            type="number"
            min={0}
            value={height}
            disabled={busy}
            onChange={(event) => setHeight(event.target.value)}
          />
        </Field>
        <div className="flex items-end">
          <PrimaryButton disabled={busy} onClick={prepareRescan} className="w-full sm:w-auto">
            Prepare rescan
          </PrimaryButton>
        </div>
      </div>
      <div className="mt-4 flex flex-col gap-3 sm:flex-row">
        <DangerButton disabled={busy} onClick={() => actions.resetLocalSync()}>
          Reset local sync only
        </DangerButton>
      </div>
    </Card>
  );
}
