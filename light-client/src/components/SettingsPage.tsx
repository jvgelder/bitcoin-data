import { useMemo, useState } from 'react';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import type { FiatCurrency } from '../state/types';
import type { BlockProviderConfig } from '../api/blockProviderApi';
import { enabledProviders } from '../state/lightClientState';
import { BlockTable } from './BlockTable';
import { EventLog } from './EventLog';
import { ProfilePanel } from './ProfilePanel';
import { ServerPanel } from './ServerPanel';
import { StatsCards } from './StatsCards';
import { SyncPanel } from './SyncPanel';
import { Card, DangerButton, Field, Input, PrimaryButton, Select } from './ui';

export function SettingsPage() {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const busy = state.status === 'syncing' || state.status === 'connecting';
  const [height, setHeight] = useState(String(state.settings.rescanHeight));
  const [currency, setCurrency] = useState<FiatCurrency>(state.wallet.selectedFiat);
  const [rate, setRate] = useState(String(state.wallet.fiatRatePerBtc || ''));
  const blockProviders = useMemo<BlockProviderConfig[]>(() => enabledProviders(state.providers.blockProviders, state.providers.activeBlockProviderId).map((provider) => ({
    name: provider.name,
    url: provider.url,
    apiKey: provider.apiKey,
    apiKeyPlacement: provider.apiKeyPlacement,
    apiKeyName: provider.apiKeyName,
  })), [state.providers]);

  function prepareRescan() {
    const parsed = Number(height);
    if (!Number.isSafeInteger(parsed) || parsed < 0) return;
    actions.rescanFromHeight(parsed);
  }

  function saveFiat() {
    actions.setFiatDisplay(currency, Number(rate || 0));
  }

  return (
    <div className="space-y-5">
      <ServerPanel />
      <StatsCards />
      <Card title="Wallet settings" subtitle="Rescan and display settings.">
        <div className="grid gap-4 sm:grid-cols-[1fr_auto]">
          <Field label="Rescan from height" hint="Resets local block summaries and prepares the next sync from this height.">
            <Input type="number" min={0} value={height} disabled={busy} onChange={(event) => setHeight(event.target.value)} />
          </Field>
          <div className="flex items-end"><PrimaryButton disabled={busy} onClick={prepareRescan}>Prepare rescan</PrimaryButton></div>
        </div>
        <div className="mt-4 grid gap-4 sm:grid-cols-[1fr_1fr_auto]">
          <Field label="Selected fiat">
            <Select value={currency} onChange={(event) => setCurrency(event.target.value as FiatCurrency)}>
              <option value="EUR">Euro</option>
              <option value="USD">US dollar</option>
              <option value="GBP">British pound</option>
              <option value="CHF">Swiss franc</option>
            </Select>
          </Field>
          <Field label="Manual BTC fiat rate" hint="No price API is queried by this prototype.">
            <Input type="number" min={0} step="0.01" value={rate} onChange={(event) => setRate(event.target.value)} placeholder="e.g. 100000" />
          </Field>
          <div className="flex items-end"><PrimaryButton onClick={saveFiat}>Save fiat</PrimaryButton></div>
        </div>
        <div className="mt-4 flex flex-col gap-3 sm:flex-row">
          <DangerButton disabled={busy} onClick={() => actions.resetLocalSync()}>Reset local sync only</DangerButton>
          <DangerButton disabled={busy} onClick={() => actions.clearWalletKey()}>Clear wallet</DangerButton>
        </div>
      </Card>
      <ProfilePanel />
      <SyncPanel />
      <BlockTable />
      <EventLog />
    </div>
  );
}
