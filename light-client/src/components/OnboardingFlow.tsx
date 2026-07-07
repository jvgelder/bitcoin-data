import { useMemo, useState } from 'react';
import { downloadWalletBackup, parseWalletHistoryImportAsync } from '../keys/backup';
import {
  generateFullPrivateKeyMaterial,
  importDescriptor,
  importWalletKeyMaterial,
  parseWalletKeyText,
  type WalletKeyMaterial,
} from '../keys/walletKeys';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { buildDemoWalletData } from '../demo/demoWallet';
import type { BlockProviderConfig } from '../api/blockProviderApi';
import { enabledProviders } from '../state/lightClientState';
import { Badge, Button, Card, Field, Input, PrimaryButton, Textarea, Toggle } from './ui';

interface OnboardingFlowProps {
  onComplete(): void;
  onImport(): void;
}

type OnboardingChoice = 'generate' | 'watch-only' | 'descriptor' | 'history';

export function OnboardingFlow({ onComplete, onImport }: OnboardingFlowProps) {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const [choice, setChoice] = useState<OnboardingChoice>();
  const [pendingKey, setPendingKey] = useState<WalletKeyMaterial>();
  const [needsCutthroughChoice, setNeedsCutthroughChoice] = useState(false);
  const [needsOfflineDate, setNeedsOfflineDate] = useState(false);
  const [demoBusy, setDemoBusy] = useState(false);
  const [demoMessage, setDemoMessage] = useState<string>();
  const blockProviders = useMemo<BlockProviderConfig[]>(() => enabledProviders(state.providers.blockProviders, state.providers.activeBlockProviderId).map((provider) => ({
    name: provider.name,
    url: provider.url,
    apiKey: provider.apiKey,
    apiKeyPlacement: provider.apiKeyPlacement,
    apiKeyName: provider.apiKeyName,
  })), [state.providers]);

  function completeGenerated(keyMaterial: WalletKeyMaterial) {
    actions.setWalletKey(keyMaterial);
    actions.completeWalletSetup({ setupKind: 'generated', balanceSat: 0, importedHistory: false });
    onComplete();
  }

  function continueWithImportedKey(keyMaterial: WalletKeyMaterial, setupKind: 'watch-only' | 'descriptor') {
    actions.setWalletKey(keyMaterial);
    setPendingKey(keyMaterial);
    setNeedsCutthroughChoice(true);
  }

  function finishCutthrough(applyCutthrough: boolean) {
    if (applyCutthrough) {
      actions.setProfile({ cutthrough: true, cutthroughStart: state.settings.startHeight });
    } else {
      actions.setProfile({ cutthrough: false, cutthroughStart: undefined, cutthroughTip: undefined });
    }
    actions.completeWalletSetup({ setupKind: choice === 'descriptor' ? 'descriptor' : 'watch-only', importedHistory: false });
    setNeedsCutthroughChoice(false);
    setPendingKey(undefined);
    onComplete();
  }

  function completeHistoryImport() {
    actions.completeWalletSetup({ setupKind: 'history-import', importedHistory: true });
    onComplete();
  }

  async function enableDemoMode() {
    setDemoBusy(true);
    setDemoMessage(undefined);
    try {
      const demoData = await buildDemoWalletData(blockProviders);
      actions.setDemoMode(true, demoData);
      onComplete();
    } catch (error) {
      setDemoMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setDemoBusy(false);
    }
  }

  if (needsCutthroughChoice && pendingKey) {
    return <CutthroughChoice keyMaterial={pendingKey} onBack={() => setNeedsCutthroughChoice(false)} onContinue={finishCutthrough} />;
  }

  if (needsOfflineDate) {
    return <OfflineDateStep onBack={() => setNeedsOfflineDate(false)} onContinue={completeHistoryImport} />;
  }

  return (
    <div className="space-y-5">
      <Card title="Start wallet setup" subtitle="Choose the key/import flow first. Sync and wallet screens appear after setup.">
        <div className="grid gap-3 md:grid-cols-2">
          <ChoiceButton
            active={choice === 'generate'}
            title="Generate Silent Payment address"
            badge="1.a"
            description="Create a new private scan key and private spend key, then immediately download a backup."
            onClick={() => setChoice('generate')}
          />
          <ChoiceButton
            active={choice === 'watch-only'}
            title="Watch-only scan wallet"
            badge="1.b"
            description="Import private scan key plus public spend key. Show balances; signing can happen elsewhere."
            onClick={() => setChoice('watch-only')}
          />
          <ChoiceButton
            active={choice === 'descriptor'}
            title="Import backup or SP descriptor"
            badge="1.c"
            description="Paste a generated backup, encrypted backup, BIP392-style sp(...) descriptor, or private scan/private spend key material."
            onClick={() => setChoice('descriptor')}
          />
          <ChoiceButton
            active={choice === 'history'}
            title="Import existing wallet history"
            badge="1.d"
            description="Import a wallet backup containing labels, scanned UTXOs/history, and optionally last online height."
            onClick={() => setChoice('history')}
          />
        </div>
      </Card>

      <Card title="Demo mode" subtitle="Open the wallet with example transactions and fake spendable UTXOs for testing send, PSBT, transaction detail, and RBF screens.">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-sm text-slate-400">No real keys are needed. Demo data can fetch a few public transactions from your Esplora provider and falls back to local examples.</p>
          <PrimaryButton disabled={demoBusy} onClick={() => void enableDemoMode()}>Enable demo mode</PrimaryButton>
        </div>
        {demoMessage ? <p className="mt-3 text-sm text-rose-200">{demoMessage}</p> : null}
      </Card>

      {choice === 'generate' ? <GenerateCard onGenerated={completeGenerated} /> : null}
      {choice === 'watch-only' ? <WatchOnlyCard onLoaded={(key) => continueWithImportedKey(key, 'watch-only')} /> : null}
      {choice === 'descriptor' ? <DescriptorCard onLoaded={(key) => continueWithImportedKey(key, 'descriptor')} /> : null}
      {choice === 'history' ? (
        <HistoryImportCard
          onImported={(requiresDate) => {
            if (requiresDate) setNeedsOfflineDate(true);
            else completeHistoryImport();
          }}
          onOpenImportPage={onImport}
        />
      ) : null}

    </div>
  );
}

function ChoiceButton({ active, title, badge, description, onClick }: { active: boolean; title: string; badge: string; description: string; onClick(): void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-2xl border p-4 text-left transition ${active ? 'border-indigo-400 bg-indigo-950/50' : 'border-slate-800 bg-slate-950/40 hover:border-slate-600'}`}
    >
      <div className="mb-3 flex items-center justify-between gap-2">
        <h3 className="font-semibold text-slate-100">{title}</h3>
        <Badge tone="indigo">{badge}</Badge>
      </div>
      <p className="text-sm leading-6 text-slate-400">{description}</p>
    </button>
  );
}

function GenerateCard({ onGenerated }: { onGenerated(keyMaterial: WalletKeyMaterial): void }) {
  const [encryptBackup, setEncryptBackup] = useState(true);
  const [password, setPassword] = useState('');
  const [message, setMessage] = useState<string>();

  async function generate() {
    if (encryptBackup && !password) {
      setMessage('Enter a backup passphrase or disable encryption.');
      return;
    }
    try {
      const keyMaterial = generateFullPrivateKeyMaterial();
      await downloadWalletBackup(keyMaterial, encryptBackup ? password : undefined);
      onGenerated(keyMaterial);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <Card title="Generate new Silent Payment wallet" subtitle="Creates private scan + private spend. A backup is downloaded before the wallet opens.">
      <div className="grid gap-4 sm:grid-cols-2">
        <Toggle checked={encryptBackup} onChange={setEncryptBackup} label="Encrypt downloaded backup" />
        <Field label="Backup passphrase" hint="Required when encrypted backup is enabled.">
          <Input type="password" value={password} onChange={(event) => setPassword(event.target.value)} />
        </Field>
      </div>
      <div className="mt-5 flex justify-end">
        <PrimaryButton onClick={() => void generate()}>Generate and download backup</PrimaryButton>
      </div>
      {message ? <p className="mt-4 text-sm text-rose-200">{message}</p> : null}
    </Card>
  );
}

function WatchOnlyCard({ onLoaded }: { onLoaded(keyMaterial: WalletKeyMaterial): void }) {
  const [privateScanKey, setPrivateScanKey] = useState('');
  const [spendPublicKey, setSpendPublicKey] = useState('');
  const [message, setMessage] = useState<string>();

  function load() {
    try {
      onLoaded(importWalletKeyMaterial({ privateScanKey, spendPublicKey }));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <Card title="Import watch-only key material" subtitle="This mode can scan and show balances. It cannot sign because the private spend key is absent.">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Private scan key">
          <Input value={privateScanKey} onChange={(event) => setPrivateScanKey(event.target.value)} placeholder="32-byte hex" />
        </Field>
        <Field label="Public spend key">
          <Input value={spendPublicKey} onChange={(event) => setSpendPublicKey(event.target.value)} placeholder="x-only or compressed hex" />
        </Field>
      </div>
      <div className="mt-5 flex justify-end"><PrimaryButton onClick={load}>Continue</PrimaryButton></div>
      {message ? <p className="mt-4 text-sm text-rose-200">{message}</p> : null}
    </Card>
  );
}

function DescriptorCard({ onLoaded }: { onLoaded(keyMaterial: WalletKeyMaterial): void }) {
  const actions = useLightClientActions();
  const [descriptor, setDescriptor] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [message, setMessage] = useState<string>();

  async function load() {
    try {
      const trimmed = descriptor.trim();
      if (trimmed.startsWith('sp(')) {
        onLoaded(importDescriptor(trimmed));
        return;
      }
      const imported = await parseWalletHistoryImportAsync(trimmed, passphrase || undefined);
      if (imported.labels || imported.transactions || imported.wallet) {
        actions.importWalletHistory(imported);
      }
      if (imported.keyMaterial) {
        onLoaded(imported.keyMaterial);
        return;
      }
      onLoaded(importWalletKeyMaterial(parseWalletKeyText(trimmed)));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <Card title="Import backup, descriptor, or private SP key" subtitle="Generated backups from the start screen can be pasted here, including encrypted backups with their passphrase.">
      <div className="space-y-4">
        <Textarea
          rows={7}
          value={descriptor}
          onChange={(event) => setDescriptor(event.target.value)}
          placeholder={'Paste generated backup text, encrypted backup JSON, sp(spspend1q...), or private_scan_key=...'}
        />
        <Field label="Backup passphrase" hint="Only required when the pasted generated backup is encrypted.">
          <Input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} placeholder="Optional" />
        </Field>
        <div className="flex justify-end"><PrimaryButton onClick={() => void load()}>Continue</PrimaryButton></div>
        {message ? <p className="text-sm text-rose-200">{message}</p> : null}
      </div>
    </Card>
  );
}

function HistoryImportCard({ onImported, onOpenImportPage }: { onImported(requiresOfflineDate: boolean): void; onOpenImportPage(): void }) {
  const actions = useLightClientActions();
  const [text, setText] = useState('');
  const [message, setMessage] = useState<string>();

  async function importHistory() {
    try {
      const imported = await parseWalletHistoryImportAsync(text);
      actions.importWalletHistory(imported);
      const lastOnlineHeight = imported.wallet?.lastOnlineHeight;
      onImported(lastOnlineHeight == null);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <Card title="Import existing wallet with history" subtitle="Paste a JSON backup with transactions, labels, optional key material, and optional last online height.">
      <Textarea rows={8} value={text} onChange={(event) => setText(event.target.value)} placeholder={'{\n  "format": "bitcoindata-sp-light-wallet-backup",\n  "wallet": { "last_online_height": 840000 },\n  "transactions": []\n}'} />
      <div className="mt-5 flex flex-col gap-3 sm:flex-row sm:justify-end">
        <Button onClick={onOpenImportPage}>Open full import page</Button>
        <PrimaryButton onClick={() => void importHistory()}>Import wallet history</PrimaryButton>
      </div>
      {message ? <p className="mt-4 text-sm text-rose-200">{message}</p> : null}
    </Card>
  );
}

function CutthroughChoice({ keyMaterial, onBack, onContinue }: { keyMaterial: WalletKeyMaterial; onBack(): void; onContinue(applyCutthrough: boolean): void }) {
  const [enabled, setEnabled] = useState(true);
  return (
    <Card title="Apply cut-through?" subtitle="Choose this after importing existing scan key material.">
      <div className="space-y-4">
        <div className="rounded-xl border border-slate-800 bg-slate-950/50 p-4 text-sm text-slate-400">
          Loaded <span className="font-medium text-slate-100">{keyMaterial.mode}</span> key material. Cut-through can save bandwidth by ignoring wallet outputs created and spent inside the selected sync window.
        </div>
        <Toggle
          checked={enabled}
          onChange={setEnabled}
          label="Apply cut-through"
          tooltip="When enabled, the light request can omit outputs that were created and spent inside the selected window. Use it when you do not need historical short-lived outputs from before the chosen scan start."
        />
        <div className="flex flex-col gap-3 sm:flex-row sm:justify-end">
          <Button onClick={onBack}>Back</Button>
          <PrimaryButton onClick={() => onContinue(enabled)}>Open wallet</PrimaryButton>
        </div>
      </div>
    </Card>
  );
}

function OfflineDateStep({ onBack, onContinue }: { onBack(): void; onContinue(): void }) {
  const actions = useLightClientActions();
  const [dateTime, setDateTime] = useState('');
  const [message, setMessage] = useState<string>();

  function continueFromDate() {
    if (!dateTime) {
      setMessage('Choose the last date/time the wallet was online.');
      return;
    }
    const iso = new Date(dateTime).toISOString();
    const estimatedHeight = estimateHeightFromDate(iso);
    actions.setSettings({ startHeight: estimatedHeight, rescanHeight: estimatedHeight });
    actions.setProfile({ cutthrough: true, cutthroughStart: estimatedHeight });
    actions.importWalletHistory({ wallet: { lastOnlineAt: iso, lastOnlineHeight: undefined } });
    onContinue();
  }

  return (
    <Card title="When was this wallet last online?" subtitle="The backup did not contain a last online height. Choose the last time you know it was synced.">
      <Field label="Last online date and time" hint="The app estimates a block height near this time and applies cut-through automatically.">
        <Input type="datetime-local" value={dateTime} onChange={(event) => setDateTime(event.target.value)} />
      </Field>
      <div className="mt-5 flex flex-col gap-3 sm:flex-row sm:justify-end">
        <Button onClick={onBack}>Back</Button>
        <PrimaryButton onClick={continueFromDate}>Continue with cut-through</PrimaryButton>
      </div>
      {message ? <p className="mt-4 text-sm text-rose-200">{message}</p> : null}
    </Card>
  );
}

function estimateHeightFromDate(iso: string): number {
  const genesis = Date.UTC(2009, 0, 3, 18, 15, 5);
  const target = new Date(iso).getTime();
  const tenMinutes = 10 * 60 * 1000;
  return Math.max(0, Math.floor((target - genesis) / tenMinutes));
}
