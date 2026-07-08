import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { Card, DangerButton, Field, Input, PrimaryButton, Button } from './ui';

export function SyncPanel() {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const busy = state.status === 'syncing' || state.status === 'connecting';
  const nextHeight = state.local.lastHeight == null ? state.settings.startHeight : state.local.lastHeight + 1;

  return (
    <Card title="Sync" subtitle="Fetches /blocks/light?start={height}&count={n} using JSON decoding for the web prototype.">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Start height">
          <Input
            type="number"
            min={0}
            value={state.settings.startHeight}
            disabled={busy}
            onChange={(event) => actions.setSettings({ startHeight: Number(event.target.value) })}
          />
        </Field>
        <Field label="Range count">
          <Input
            type="number"
            min={1}
            value={state.settings.rangeCount}
            disabled={busy}
            onChange={(event) => actions.setSettings({ rangeCount: Math.max(1, Number(event.target.value)) })}
          />
        </Field>
      </div>

      <div className="mt-5 flex flex-col gap-3 sm:flex-row">
        <PrimaryButton
          disabled={busy}
          onClick={() => void actions.syncRange(state.settings.startHeight, state.settings.rangeCount)}
        >
          Sync bounded range
        </PrimaryButton>
        <Button disabled={busy} onClick={() => void actions.syncRange(nextHeight, state.settings.rangeCount)}>
          Sync next range
        </Button>
        <Button disabled={busy} onClick={() => void actions.syncToTip()}>
          Sync until tip
        </Button>
        {busy ? <DangerButton onClick={() => actions.stopSync()}>Stop</DangerButton> : null}
      </div>
    </Card>
  );
}
