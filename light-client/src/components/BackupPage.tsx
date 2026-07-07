import { useState } from 'react';
import { downloadWalletStateBackup } from '../keys/backup';
import { useLightClientState } from '../state/LightClientProvider';
import { Button, Card, Field, Input, PrimaryButton, Toggle } from './ui';

export function BackupPage() {
  const state = useLightClientState();
  const [password, setPassword] = useState('');
  const [encrypt, setEncrypt] = useState(true);
  const [message, setMessage] = useState<{ tone: 'success' | 'error'; text: string }>();

  async function exportBackup() {
    if (encrypt && !password) {
      setMessage({ tone: 'error', text: 'Enter a passphrase or disable encryption.' });
      return;
    }
    try {
      await downloadWalletStateBackup(state, encrypt ? password : undefined);
      setMessage({ tone: 'success', text: 'Backup exported.' });
    } catch (error) {
      setMessage({ tone: 'error', text: error instanceof Error ? error.message : String(error) });
    }
  }

  return (
    <Card title="Backup" subtitle="Export key material, labels, sync metadata, and imported history. Store backups offline.">
      <div className="space-y-4">
        <Toggle checked={encrypt} onChange={setEncrypt} label="Encrypt backup" />
        <Field label="Backup passphrase">
          <Input type="password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder={encrypt ? 'Required' : 'Optional'} />
        </Field>
        <div className="flex justify-end gap-3">
          <Button onClick={() => setPassword('')}>Clear passphrase</Button>
          <PrimaryButton onClick={() => void exportBackup()}>Export backup</PrimaryButton>
        </div>
        {message ? <p className={message.tone === 'success' ? 'text-sm text-emerald-200' : 'text-sm text-rose-200'}>{message.text}</p> : null}
      </div>
    </Card>
  );
}
