import type { LightClientState, WalletLabel, WalletSpendableUtxo, WalletSummaryState, WalletTransaction } from '../state/types';
import type { WalletKeyMaterial } from './walletKeys';
import { bytesToHex, hexToBytes, importWalletKeyMaterial, parseWalletKeyText } from './walletKeys';

const BACKUP_VERSION = 2;
const PBKDF2_ITERATIONS = 250_000;

export interface WalletHistoryImport {
  wallet?: Partial<WalletSummaryState>;
  labels?: WalletLabel[];
  transactions?: WalletTransaction[];
  spendableUtxos?: WalletSpendableUtxo[];
  keyMaterial?: WalletKeyMaterial;
}

export async function downloadWalletBackup(keyMaterial: WalletKeyMaterial, password?: string): Promise<void> {
  const plainText = backupPlainText(keyMaterial);
  const encrypted = password?.length ? await encryptBackupText(plainText, password) : plainText;
  const filename = backupFilename(keyMaterial, Boolean(password?.length));
  downloadTextFile(filename, encrypted);
}

export async function downloadWalletStateBackup(state: LightClientState, password?: string): Promise<void> {
  if (!state.walletKey) throw new Error('Load or generate wallet key material first.');
  const plainText = walletStateBackupJson(state);
  const encrypted = password?.length ? await encryptBackupText(plainText, password) : plainText;
  const date = new Date().toISOString().slice(0, 10);
  const suffix = password?.length ? 'encrypted' : 'plain';
  downloadTextFile(`silent-payment-wallet-backup-${date}-${suffix}.json`, encrypted);
}

export function backupPlainText(keyMaterial: WalletKeyMaterial): string {
  const lines = [
    '# bitcoindata Silent Payments light-client backup',
    `version=${BACKUP_VERSION}`,
    `created_at=${keyMaterial.createdAt}`,
    `mode=${keyMaterial.mode}`,
    keyMaterial.descriptor ? `descriptor=${keyMaterial.descriptor}` : undefined,
    `private_scan_key=${keyMaterial.privateScanKey}`,
    `scan_public_key=${keyMaterial.scanPublicKey ?? ''}`,
    `scan_public_key_xonly=${keyMaterial.scanPublicKeyXOnly ?? ''}`,
    `spend_public_key=${keyMaterial.spendPublicKey}`,
    `spend_public_key_xonly=${keyMaterial.spendPublicKeyXOnly}`,
  ].filter(Boolean) as string[];

  if (keyMaterial.privateSpendKey) {
    lines.push(`private_spend_key=${keyMaterial.privateSpendKey}`);
  } else {
    lines.push('# private_spend_key is intentionally absent for watch-only mode');
  }

  lines.push('', '# Import formats accepted by this UI: key=value text, JSON, or BIP392 sp(...) descriptors.', '');
  return `${lines.join('\n')}\n`;
}

export function walletStateBackupJson(state: LightClientState): string {
  const payload = {
    format: 'bitcoindata-sp-light-wallet-backup',
    version: BACKUP_VERSION,
    created_at: new Date().toISOString(),
    network: state.manifest?.network,
    wallet: {
      setup_kind: state.wallet.setupKind,
      balance_sat: state.spendableUtxos.filter((utxo) => utxo.confirmed && !utxo.reservedByTxid).reduce((sum, utxo) => sum + utxo.valueSat, 0),
      selected_fiat: state.wallet.selectedFiat,
      fiat_rate_per_btc: state.wallet.fiatRatePerBtc,
      last_online_height: state.wallet.lastOnlineHeight,
      last_online_at: state.wallet.lastOnlineAt,
      imported_history: state.wallet.importedHistory,
      descriptor: state.walletKey?.descriptor ?? state.wallet.descriptor,
    },
    key_material: state.walletKey
      ? {
          mode: state.walletKey.mode,
          descriptor: state.walletKey.descriptor,
          private_scan_key: state.walletKey.privateScanKey,
          scan_public_key: state.walletKey.scanPublicKey,
          scan_public_key_xonly: state.walletKey.scanPublicKeyXOnly,
          spend_public_key: state.walletKey.spendPublicKey,
          spend_public_key_xonly: state.walletKey.spendPublicKeyXOnly,
          private_spend_key: state.walletKey.privateSpendKey,
        }
      : undefined,
    scan_state: {
      last_height: state.local.lastHeight,
      last_block_hash: state.local.lastBlockHash,
      start_height: state.settings.startHeight,
    },
    labels: state.labels.map((label) => ({
      id: label.id,
      path: `/${label.path.join('/')}`,
      created_at: label.createdAt,
      updated_at: label.updatedAt,
    })),
    spendable_utxos: state.spendableUtxos.map((utxo) => ({
      id: utxo.id,
      txid: utxo.txid,
      vout: utxo.vout,
      value_sat: utxo.valueSat,
      script_pubkey: utxo.scriptPubKey,
      confirmed: utxo.confirmed,
      label_id: utxo.labelId,
      reserved_by_txid: utxo.reservedByTxid,
      derived_private_key: utxo.derivedPrivateKey,
    })),
    transactions: state.transactions.map((tx) => ({
      id: tx.id,
      txid: tx.txid,
      direction: tx.direction,
      amount_sat: tx.amountSat,
      fee_sat: tx.feeSat,
      date_time: tx.dateTime,
      label_id: tx.labelId,
      confirmations: tx.confirmations,
      note: tx.note,
    })),
  };
  return `${JSON.stringify(payload, null, 2)}\n`;
}

export async function parseWalletHistoryImportAsync(text: string, password?: string): Promise<WalletHistoryImport> {
  const plainText = await maybeDecryptBackupText(text, password);
  return parseWalletHistoryImport(plainText);
}

export async function maybeDecryptBackupText(text: string, password?: string): Promise<string> {
  const trimmed = text.trim();
  if (!trimmed.startsWith('{')) return text;
  const parsed = JSON.parse(trimmed) as Record<string, unknown>;
  if (parsed.format !== 'bitcoindata-sp-light-client-encrypted-backup') return text;
  if (!password) throw new Error('This backup is encrypted. Enter the passphrase used when it was created.');
  return decryptBackupText(parsed, password);
}

export async function decryptBackupText(parsed: Record<string, unknown>, password: string): Promise<string> {
  const salt = hexToBytes(requiredString(parsed.salt, 'salt'));
  const iv = hexToBytes(requiredString(parsed.iv, 'iv'));
  const ciphertext = hexToBytes(requiredString(parsed.ciphertext, 'ciphertext'));
  const key = await deriveAesKey(password, salt);
  const decrypted = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: toArrayBuffer(iv) }, key, toArrayBuffer(ciphertext));
  return new TextDecoder().decode(decrypted);
}

export function parseWalletHistoryImport(text: string): WalletHistoryImport {
  const trimmed = text.trim();
  if (!trimmed) throw new Error('Paste a wallet backup, descriptor, or key material first.');

  if (trimmed.startsWith('sp(') || !trimmed.startsWith('{')) {
    const keyArgs = parseWalletKeyText(trimmed);
    return { keyMaterial: importWalletKeyMaterial(keyArgs) };
  }

  const parsed = JSON.parse(trimmed) as Record<string, unknown>;
  const keyMaterial = parseOptionalKeyMaterial(parsed);
  const wallet = parseWalletMetadata(parsed);
  const labels = parseLabels(parsed.labels);
  const transactions = parseTransactions(parsed.transactions);
  const spendableUtxos = parseSpendableUtxos(parsed.spendable_utxos ?? parsed.spendableUtxos ?? parsed.utxos);

  return { wallet, labels, transactions, spendableUtxos, keyMaterial };
}

export async function encryptBackupText(plainText: string, password: string): Promise<string> {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveAesKey(password, salt);
  const encoded = new TextEncoder().encode(plainText);
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: toArrayBuffer(iv) }, key, toArrayBuffer(encoded)));
  const payload = {
    format: 'bitcoindata-sp-light-client-encrypted-backup',
    version: BACKUP_VERSION,
    kdf: 'PBKDF2-HMAC-SHA256',
    iterations: PBKDF2_ITERATIONS,
    cipher: 'AES-256-GCM',
    salt: bytesToHex(salt),
    iv: bytesToHex(iv),
    ciphertext: bytesToHex(ciphertext),
  };
  return `${JSON.stringify(payload, null, 2)}\n`;
}

function parseOptionalKeyMaterial(parsed: Record<string, unknown>): WalletKeyMaterial | undefined {
  try {
    const keySource = parsed.key_material && typeof parsed.key_material === 'object'
      ? (parsed.key_material as Record<string, unknown>)
      : parsed;
    const descriptor = asString(keySource.descriptor ?? parsed.descriptor);
    if (descriptor) return importWalletKeyMaterial(parseWalletKeyText(descriptor), 'imported');
    const privateScanKey = asString(keySource.private_scan_key ?? keySource.privateScanKey);
    const spendPublicKey = asString(keySource.spend_public_key ?? keySource.spendPublicKey);
    const privateSpendKey = asString(keySource.private_spend_key ?? keySource.privateSpendKey);
    if (!privateScanKey) return undefined;
    return importWalletKeyMaterial({ privateScanKey, spendPublicKey, privateSpendKey }, 'imported');
  } catch {
    return undefined;
  }
}

function parseWalletMetadata(parsed: Record<string, unknown>): Partial<WalletSummaryState> {
  const wallet = parsed.wallet && typeof parsed.wallet === 'object' ? (parsed.wallet as Record<string, unknown>) : parsed;
  const scanState = parsed.scan_state && typeof parsed.scan_state === 'object' ? (parsed.scan_state as Record<string, unknown>) : undefined;
  const balanceSat = asNumber(wallet.balance_sat ?? wallet.balanceSat);
  const lastOnlineHeight = asNumber(wallet.last_online_height ?? wallet.lastOnlineHeight ?? scanState?.last_height);
  const lastOnlineAt = asString(wallet.last_online_at ?? wallet.lastOnlineAt ?? wallet.iso_8601_datetime ?? wallet.timestamp);
  const descriptor = asString(wallet.descriptor ?? parsed.descriptor);

  return {
    setupKind: 'history-import',
    setupComplete: false,
    importedHistory: true,
    balanceSat: balanceSat ?? undefined,
    lastOnlineHeight,
    lastOnlineAt,
    descriptor,
  };
}

function parseLabels(input: unknown): WalletLabel[] | undefined {
  if (!Array.isArray(input)) return undefined;
  const now = new Date().toISOString();
  return input
    .map((item, index): WalletLabel | undefined => {
      if (!item || typeof item !== 'object') return undefined;
      const record = item as Record<string, unknown>;
      const id = asNumber(record.id ?? record.label_id) ?? index + 1;
      const rawPath = asString(record.path ?? record.label ?? record.name) ?? `/label-${id}`;
      return {
        id,
        path: rawPath.split('/').map((part) => part.trim()).filter(Boolean),
        createdAt: asString(record.created_at ?? record.createdAt) ?? now,
        updatedAt: asString(record.updated_at ?? record.updatedAt) ?? now,
      };
    })
    .filter((label): label is WalletLabel => Boolean(label));
}

function parseTransactions(input: unknown): WalletTransaction[] | undefined {
  if (!Array.isArray(input)) return undefined;
  return input
    .map((item, index): WalletTransaction | undefined => {
      if (!item || typeof item !== 'object') return undefined;
      const record = item as Record<string, unknown>;
      const txid = asString(record.txid ?? record.transaction_id);
      const amountSat = asNumber(record.amount_sat ?? record.amountSat ?? record.value_sat ?? record.valueSat);
      if (!txid || amountSat == null) return undefined;
      const direction = record.direction === 'sent' || record.direction === 'received' ? record.direction : amountSat < 0 ? 'sent' : 'received';
      return {
        id: asString(record.id) ?? `${txid}-${index}`,
        txid,
        direction,
        amountSat: Math.abs(amountSat),
        feeSat: asNumber(record.fee_sat ?? record.feeSat),
        dateTime: asString(record.date_time ?? record.dateTime ?? record.datetime ?? record.date) ?? new Date().toISOString(),
        labelId: asNumber(record.label_id ?? record.labelId) ?? 1,
        confirmations: asNumber(record.confirmations),
        note: asString(record.note),
      };
    })
    .filter((tx): tx is WalletTransaction => Boolean(tx));
}


function parseSpendableUtxos(input: unknown): WalletSpendableUtxo[] | undefined {
  if (!Array.isArray(input)) return undefined;
  return input
    .map((item, index): WalletSpendableUtxo | undefined => {
      if (!item || typeof item !== 'object') return undefined;
      const record = item as Record<string, unknown>;
      const txid = asString(record.txid);
      const vout = asNumber(record.vout);
      const valueSat = asNumber(record.value_sat ?? record.valueSat ?? record.value);
      const scriptPubKey = asString(record.script_pubkey ?? record.scriptPubKey);
      if (!txid || vout == null || valueSat == null || !scriptPubKey) return undefined;
      return {
        id: asString(record.id) ?? `${txid}:${vout}`,
        txid,
        vout,
        valueSat,
        scriptPubKey,
        confirmed: record.confirmed !== false,
        labelId: asNumber(record.label_id ?? record.labelId) ?? 1,
        reservedByTxid: asString(record.reserved_by_txid ?? record.reservedByTxid),
        derivedPrivateKey: asString(record.derived_private_key ?? record.derivedPrivateKey),
      };
    })
    .filter((utxo): utxo is WalletSpendableUtxo => Boolean(utxo));
}

function backupFilename(keyMaterial: WalletKeyMaterial, encrypted: boolean): string {
  const date = new Date().toISOString().slice(0, 10);
  const suffix = encrypted ? 'encrypted' : 'plain';
  return `silent-payment-light-client-${keyMaterial.mode}-${date}-${suffix}.txt`;
}

async function deriveAesKey(password: string, salt: Uint8Array): Promise<CryptoKey> {
  const baseKey = await crypto.subtle.importKey('raw', new TextEncoder().encode(password), 'PBKDF2', false, [
    'deriveKey',
  ]);
  return crypto.subtle.deriveKey(
    {
      name: 'PBKDF2',
      hash: 'SHA-256',
      salt: toArrayBuffer(salt),
      iterations: PBKDF2_ITERATIONS,
    },
    baseKey,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  );
}

function downloadTextFile(filename: string, text: string): void {
  const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

function requiredString(value: unknown, label: string): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`Encrypted backup missing ${label}.`);
  return value.trim();
}

function asString(value: unknown): string | undefined {
  if (typeof value === 'number') return String(value);
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function asNumber(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}
