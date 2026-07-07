import { useMemo, useState } from 'react';
import { downloadWalletBackup } from '../keys/backup';
import {
  generateFullPrivateKeyMaterial,
  importWalletKeyMaterial,
  parseWalletKeyText,
  type WalletKeyMaterial,
} from '../keys/walletKeys';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { formatDateTime, shortHash } from '../utils/format';
import { Badge, Button, Card, DangerButton, Field, Input, PrimaryButton, Textarea, Toggle } from './ui';

export function WalletKeysPanel() {
  const { walletKey } = useLightClientState();
  const actions = useLightClientActions();
  const [privateScanKey, setPrivateScanKey] = useState('');
  const [spendPublicKey, setSpendPublicKey] = useState('');
  const [privateSpendKey, setPrivateSpendKey] = useState('');
  const [backupPassword, setBackupPassword] = useState('');
  const [encryptBackup, setEncryptBackup] = useState(false);
  const [importText, setImportText] = useState('');
  const [message, setMessage] = useState<{ tone: 'info' | 'error' | 'success'; text: string }>();

  const canGenerate = !encryptBackup || backupPassword.length > 0;
  const backupHint = useMemo(
    () =>
      encryptBackup
        ? 'A backup text file will be downloaded and encrypted with AES-GCM using the password below.'
        : 'A plain-text backup file will be downloaded. Store it offline; it contains private keys.',
    [encryptBackup],
  );

  async function generateKey() {
    if (!canGenerate) {
      setMessage({ tone: 'error', text: 'Enter a backup password or disable encryption.' });
      return;
    }
    try {
      const keyMaterial = generateFullPrivateKeyMaterial();
      actions.setWalletKey(keyMaterial);
      await downloadWalletBackup(keyMaterial, encryptBackup ? backupPassword : undefined);
      setMessage({ tone: 'success', text: 'Generated a full private Silent Payment key and downloaded the backup.' });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  function importWatchOnly() {
    loadKey(() => importWalletKeyMaterial({ privateScanKey, spendPublicKey }));
  }

  function importFullPrivate() {
    loadKey(() => importWalletKeyMaterial({ privateScanKey, privateSpendKey }));
  }

  function importPastedText() {
    loadKey(() => importWalletKeyMaterial(parseWalletKeyText(importText)));
  }

  function loadKey(loader: () => WalletKeyMaterial) {
    try {
      const keyMaterial = loader();
      actions.setWalletKey(keyMaterial);
      setMessage({
        tone: 'success',
        text:
          keyMaterial.mode === 'full-private'
            ? 'Loaded private scan key and private spend key.'
            : 'Loaded watch-only key material: private scan key and public spend key.',
      });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  async function downloadCurrentBackup() {
    if (!walletKey) return;
    if (encryptBackup && !backupPassword) {
      setMessage({ tone: 'error', text: 'Enter a backup password or disable encryption.' });
      return;
    }
    try {
      await downloadWalletBackup(walletKey, encryptBackup ? backupPassword : undefined);
      setMessage({ tone: 'success', text: 'Downloaded a backup for the loaded key material.' });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  return (
    <Card
      title="Silent Payment keys"
      subtitle="Use watch-only key material for scanning/balances, or load the private spend key when this UI later grows signing support."
    >
      <div className="rounded-xl border border-slate-800 bg-slate-950/50 p-4">
        {walletKey ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={walletKey.mode === 'full-private' ? 'green' : 'indigo'}>
                {walletKey.mode === 'full-private' ? 'private SP key' : 'watch-only scan key'}
              </Badge>
              <span className="text-xs text-slate-500">Loaded {formatDateTime(walletKey.createdAt)}</span>
            </div>
            <KeyLine label="Private scan" value={walletKey.privateScanKey} sensitive />
            <KeyLine label="Spend public" value={walletKey.spendPublicKey} />
            <KeyLine label="Spend x-only" value={walletKey.spendPublicKeyXOnly} />
            {walletKey.privateSpendKey ? <KeyLine label="Private spend" value={walletKey.privateSpendKey} sensitive /> : null}
            <div className="flex flex-col gap-2 sm:flex-row">
              <Button onClick={() => void downloadCurrentBackup()}>Download backup</Button>
              <DangerButton onClick={() => actions.clearWalletKey()}>Clear keys from memory</DangerButton>
            </div>
          </div>
        ) : (
          <div className="text-sm text-slate-500">No wallet key material loaded.</div>
        )}
      </div>

      <div className="mt-5 grid gap-4 md:grid-cols-2">
        <div className="rounded-xl border border-slate-800 bg-slate-950/40 p-4">
          <h3 className="text-sm font-semibold text-slate-100">Generate full private key</h3>
          <p className="mt-1 text-xs text-slate-500">Creates private scan + private spend keys, derives the public spend key, and downloads a backup.</p>
          <div className="mt-4 space-y-3">
            <Toggle checked={encryptBackup} onChange={setEncryptBackup} label="Encrypt downloaded backup" />
            <Field label="Backup password" hint={backupHint}>
              <Input
                type="password"
                value={backupPassword}
                onChange={(event) => setBackupPassword(event.target.value)}
                placeholder={encryptBackup ? 'Required for encrypted backup' : 'Optional'}
              />
            </Field>
            <PrimaryButton onClick={() => void generateKey()} disabled={!canGenerate} className="w-full">
              Generate and download backup
            </PrimaryButton>
          </div>
        </div>

        <div className="rounded-xl border border-slate-800 bg-slate-950/40 p-4">
          <h3 className="text-sm font-semibold text-slate-100">Import key material</h3>
          <p className="mt-1 text-xs text-slate-500">Watch-only mode requires private scan + public spend. Full private mode requires private scan + private spend.</p>
          <div className="mt-4 space-y-3">
            <Field label="Private scan key">
              <Input value={privateScanKey} onChange={(event) => setPrivateScanKey(event.target.value)} placeholder="32-byte hex" />
            </Field>
            <Field label="Public spend key">
              <Input value={spendPublicKey} onChange={(event) => setSpendPublicKey(event.target.value)} placeholder="32-byte x-only or 33-byte compressed hex" />
            </Field>
            <Field label="Private spend key">
              <Input value={privateSpendKey} onChange={(event) => setPrivateSpendKey(event.target.value)} placeholder="32-byte hex" />
            </Field>
            <div className="grid gap-2 sm:grid-cols-2">
              <Button onClick={importWatchOnly}>Load watch-only</Button>
              <Button onClick={importFullPrivate}>Load private SP key</Button>
            </div>
          </div>
        </div>
      </div>

      <div className="mt-5 rounded-xl border border-slate-800 bg-slate-950/40 p-4">
        <h3 className="text-sm font-semibold text-slate-100">Paste backup text</h3>
        <div className="mt-3 grid gap-3">
          <Textarea
            rows={6}
            value={importText}
            onChange={(event) => setImportText(event.target.value)}
            placeholder={'private_scan_key=...\nspend_public_key=...\n# or private_spend_key=...'}
          />
          <div className="flex justify-end">
            <Button onClick={importPastedText}>Import pasted backup</Button>
          </div>
        </div>
      </div>

      {message ? <Message tone={message.tone} text={message.text} /> : null}
    </Card>
  );
}

function KeyLine({ label, value, sensitive = false }: { label: string; value: string; sensitive?: boolean }) {
  return (
    <div className="grid gap-1 text-xs sm:grid-cols-[110px_1fr]">
      <span className="text-slate-500">{label}</span>
      <span className="break-all font-mono text-slate-300">{sensitive ? `${value.slice(0, 10)}…${value.slice(-10)}` : shortHash(value)}</span>
    </div>
  );
}

function Message({ tone, text }: { tone: 'info' | 'error' | 'success'; text: string }) {
  const classes = {
    info: 'border-slate-700 bg-slate-950/70 text-slate-200',
    error: 'border-rose-700 bg-rose-950/70 text-rose-100',
    success: 'border-emerald-700 bg-emerald-950/70 text-emerald-100',
  };
  return <div className={`mt-4 rounded-xl border p-3 text-sm ${classes[tone]}`}>{text}</div>;
}
