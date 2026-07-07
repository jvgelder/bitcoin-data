import { useState } from 'react';
import { parseWalletHistoryImportAsync } from '../keys/backup';
import { importDescriptor, importWalletKeyMaterial, parseWalletKeyText } from '../keys/walletKeys';
import { useLightClientActions } from '../state/LightClientProvider';
import { Card, Field, Input, PrimaryButton, Textarea } from './ui';

export function ImportPage({ onImported }: { onImported(): void }) {
  const actions = useLightClientActions();
  const [text, setText] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [message, setMessage] = useState<{ tone: 'success' | 'error'; text: string }>();

  async function importText() {
    try {
      const trimmed = text.trim();
      if (trimmed.startsWith('sp(')) {
        const keyMaterial = importDescriptor(trimmed);
        actions.setWalletKey(keyMaterial);
        actions.completeWalletSetup({ setupKind: 'descriptor', descriptor: keyMaterial.descriptor, importedHistory: false });
      } else {
        const imported = await parseWalletHistoryImportAsync(trimmed, passphrase || undefined);
        if (!imported.keyMaterial && !imported.wallet && !imported.transactions?.length) {
          const keyMaterial = importWalletKeyMaterial(parseWalletKeyText(trimmed));
          actions.setWalletKey(keyMaterial);
          actions.completeWalletSetup({ setupKind: keyMaterial.mode === 'watch-only' ? 'watch-only' : 'descriptor', importedHistory: false });
        } else if (imported.keyMaterial && !imported.wallet && !imported.transactions?.length && !imported.labels?.length) {
          actions.setWalletKey(imported.keyMaterial);
          actions.completeWalletSetup({
            setupKind: imported.keyMaterial.mode === 'watch-only' ? 'watch-only' : 'descriptor',
            descriptor: imported.keyMaterial.descriptor,
            importedHistory: false,
          });
        } else {
          actions.importWalletHistory(imported);
          if (imported.keyMaterial) actions.setWalletKey(imported.keyMaterial);
          actions.completeWalletSetup({ setupKind: 'history-import', importedHistory: true });
        }
      }
      setMessage({ tone: 'success', text: 'Import complete.' });
      onImported();
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  return (
    <Card title="Import" subtitle="Paste a generated backup, encrypted backup, BIP392-style sp(...) descriptor, private key material, or wallet-history JSON.">
      <div className="space-y-4">
        <Textarea
          rows={12}
          value={text}
          onChange={(event) => setText(event.target.value)}
          placeholder={'Paste generated backup text, encrypted backup JSON, sp(spscan1q...), or wallet-history JSON'}
        />
        <Field label="Backup passphrase" hint="Only required for encrypted backups created by this client.">
          <Input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} placeholder="Optional" />
        </Field>
        <div className="flex justify-end"><PrimaryButton onClick={() => void importText()}>Import</PrimaryButton></div>
        {message ? <p className={message.tone === 'success' ? 'text-sm text-emerald-200' : 'text-sm text-rose-200'}>{message.text}</p> : null}
      </div>
    </Card>
  );
}
