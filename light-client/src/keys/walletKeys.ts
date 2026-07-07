import { getPublicKey, utils as secpUtils } from '@noble/secp256k1';

export type WalletKeyMode = 'watch-only' | 'full-private' | 'descriptor-only';

export interface WalletKeyMaterial {
  mode: WalletKeyMode;
  privateScanKey: string;
  spendPublicKey: string;
  spendPublicKeyXOnly: string;
  privateSpendKey?: string;
  scanPublicKey?: string;
  scanPublicKeyXOnly?: string;
  descriptor?: string;
  createdAt: string;
  source: 'generated' | 'imported';
}

interface ImportKeyArgs {
  privateScanKey: string;
  spendPublicKey?: string;
  privateSpendKey?: string;
  descriptor?: string;
}

const BECH32M_CONST = 0x2bc830a3;
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

export function generateFullPrivateKeyMaterial(): WalletKeyMaterial {
  const privateScanKey = bytesToHex(secpUtils.randomSecretKey());
  const privateSpendKey = bytesToHex(secpUtils.randomSecretKey());
  return importWalletKeyMaterial({ privateScanKey, privateSpendKey }, 'generated');
}

export function importWalletKeyMaterial(args: ImportKeyArgs, source: 'generated' | 'imported' = 'imported'): WalletKeyMaterial {
  const privateScanKey = normalizeSecretKey(args.privateScanKey, 'private scan key');
  const scanPublicKey = bytesToHex(getPublicKey(hexToBytes(privateScanKey), true));

  if (args.privateSpendKey?.trim()) {
    const privateSpendKey = normalizeSecretKey(args.privateSpendKey, 'private spend key');
    const spendPublicKey = bytesToHex(getPublicKey(hexToBytes(privateSpendKey), true));
    return {
      mode: 'full-private',
      privateScanKey,
      privateSpendKey,
      spendPublicKey,
      spendPublicKeyXOnly: xOnlyFromPublicKey(spendPublicKey),
      scanPublicKey,
      scanPublicKeyXOnly: xOnlyFromPublicKey(scanPublicKey),
      descriptor: args.descriptor ?? descriptorFromSpspend(privateScanKey, privateSpendKey),
      createdAt: new Date().toISOString(),
      source,
    };
  }

  if (!args.spendPublicKey?.trim()) {
    throw new Error('Provide either a public spend key or a private spend key.');
  }

  const spendPublicKey = normalizePublicKey(args.spendPublicKey, 'public spend key');
  return {
    mode: 'watch-only',
    privateScanKey,
    spendPublicKey,
    spendPublicKeyXOnly: xOnlyFromPublicKey(spendPublicKey),
    scanPublicKey,
    scanPublicKeyXOnly: xOnlyFromPublicKey(scanPublicKey),
    descriptor: args.descriptor ?? descriptorFromSpscan(privateScanKey, spendPublicKey),
    createdAt: new Date().toISOString(),
    source,
  };
}

export function importDescriptor(text: string): WalletKeyMaterial {
  const descriptor = text.trim();
  if (!descriptor.startsWith('sp(') || !descriptor.endsWith(')')) {
    throw new Error('Descriptor must be a top-level sp(...) descriptor.');
  }
  const inner = descriptor.slice(3, -1).trim();
  const args = splitDescriptorArgs(inner);

  if (args.length === 1) {
    const expression = stripOrigin(args[0]);
    const decoded = decodeBip392KeyExpression(expression);
    return importWalletKeyMaterial({ ...decoded, descriptor }, 'imported');
  }

  if (args.length === 2) {
    const scan = normalizeDescriptorKeyArg(stripOrigin(args[0]));
    const spend = normalizeDescriptorKeyArg(stripOrigin(args[1]));
    if (!scan.privateKey) {
      throw new Error('The two-argument sp(scan, spend) form requires a private scan key.');
    }
    if (spend.privateKey) {
      return importWalletKeyMaterial({ privateScanKey: scan.privateKey, privateSpendKey: spend.privateKey, descriptor }, 'imported');
    }
    if (!spend.publicKey) {
      throw new Error('The spend key must be a private key or compressed public key.');
    }
    return importWalletKeyMaterial({ privateScanKey: scan.privateKey, spendPublicKey: spend.publicKey, descriptor }, 'imported');
  }

  throw new Error('sp() descriptor must contain one encoded BIP392 key or two key expressions.');
}

export function parseWalletKeyText(text: string): ImportKeyArgs {
  const trimmed = text.trim();
  if (!trimmed) throw new Error('Paste a key backup, descriptor, or key-value text first.');

  if (trimmed.startsWith('sp(')) {
    const keyMaterial = importDescriptor(trimmed);
    return {
      privateScanKey: keyMaterial.privateScanKey,
      spendPublicKey: keyMaterial.mode === 'watch-only' ? keyMaterial.spendPublicKey : undefined,
      privateSpendKey: keyMaterial.privateSpendKey,
      descriptor: keyMaterial.descriptor,
    };
  }

  if (trimmed.startsWith('{')) {
    return parseJsonKeyText(trimmed);
  }

  const fields = new Map<string, string>();
  for (const rawLine of trimmed.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const separator = line.includes('=') ? '=' : line.includes(':') ? ':' : undefined;
    if (!separator) continue;
    const [rawKey, ...rest] = line.split(separator);
    const key = normalizeFieldName(rawKey);
    const value = rest.join(separator).trim();
    if (key && value) fields.set(key, value);
  }

  const descriptor = pickField(fields, ['descriptor', 'spDescriptor', 'silentPaymentDescriptor']);
  if (descriptor) return parseWalletKeyText(descriptor);

  const privateScanKey = pickField(fields, ['privateScanKey', 'scanPrivateKey', 'private_scan_key', 'scan_private_key', 'privScan', 'scanPriv']);
  const spendPublicKey = pickField(fields, ['spendPublicKey', 'publicSpendKey', 'spend_public_key', 'public_spend_key', 'pubSpend', 'spendPub']);
  const privateSpendKey = pickField(fields, ['privateSpendKey', 'spendPrivateKey', 'private_spend_key', 'spend_private_key', 'privSpend', 'spendPriv']);

  if (!privateScanKey) {
    throw new Error('Could not find private_scan_key in the pasted text.');
  }

  return { privateScanKey, spendPublicKey, privateSpendKey };
}

export function descriptorFromSpscan(privateScanKey: string, spendPublicKey: string): string {
  const payload = concatBytes(hexToBytes(normalizeSecretKey(privateScanKey, 'private scan key')), hexToBytes(normalizePublicKey(spendPublicKey, 'public spend key')));
  return `sp(${bech32mEncode('spscan', payload)})`;
}

export function descriptorFromSpspend(privateScanKey: string, privateSpendKey: string): string {
  const payload = concatBytes(hexToBytes(normalizeSecretKey(privateScanKey, 'private scan key')), hexToBytes(normalizeSecretKey(privateSpendKey, 'private spend key')));
  return `sp(${bech32mEncode('spspend', payload)})`;
}

function parseJsonKeyText(text: string): ImportKeyArgs {
  const parsed = JSON.parse(text) as Record<string, unknown>;
  const descriptor = asString(parsed.descriptor ?? parsed.sp_descriptor ?? parsed.silent_payment_descriptor);
  if (descriptor) return parseWalletKeyText(descriptor);

  const keyMaterial = parsed.key_material && typeof parsed.key_material === 'object'
    ? (parsed.key_material as Record<string, unknown>)
    : parsed;
  const privateScanKey = asString(
    keyMaterial.private_scan_key ?? keyMaterial.privateScanKey ?? keyMaterial.scan_private_key ?? keyMaterial.scanPrivateKey,
  );
  const spendPublicKey = asString(
    keyMaterial.spend_public_key ?? keyMaterial.spendPublicKey ?? keyMaterial.public_spend_key ?? keyMaterial.publicSpendKey,
  );
  const privateSpendKey = asString(
    keyMaterial.private_spend_key ?? keyMaterial.privateSpendKey ?? keyMaterial.spend_private_key ?? keyMaterial.spendPrivateKey,
  );

  if (!privateScanKey) throw new Error('JSON backup did not contain private_scan_key or descriptor.');
  return { privateScanKey, spendPublicKey, privateSpendKey };
}

function decodeBip392KeyExpression(expression: string): ImportKeyArgs {
  const { hrp, payload } = bech32mDecode(expression);
  if (hrp === 'spscan' || hrp === 'tspscan') {
    if (payload.length !== 65) throw new Error('spscan payload must contain 32-byte scan private key and 33-byte spend public key.');
    return {
      privateScanKey: bytesToHex(payload.slice(0, 32)),
      spendPublicKey: bytesToHex(payload.slice(32)),
    };
  }
  if (hrp === 'spspend' || hrp === 'tspspend') {
    if (payload.length !== 64) throw new Error('spspend payload must contain 32-byte scan private key and 32-byte spend private key.');
    return {
      privateScanKey: bytesToHex(payload.slice(0, 32)),
      privateSpendKey: bytesToHex(payload.slice(32)),
    };
  }
  throw new Error('Expected spscan/spspend BIP392 key expression.');
}

function normalizeDescriptorKeyArg(input: string): { privateKey?: string; publicKey?: string } {
  const clean = input.trim();
  const hex = stripHex(clean);

  if (/^(02|03)[0-9a-f]{64}$/.test(hex)) {
    return { publicKey: normalizePublicKey(hex, 'descriptor public key') };
  }

  if (/^[0-9a-f]{64}$/.test(hex)) {
    if (secpUtils.isValidSecretKey(hexToBytes(hex))) {
      return { privateKey: normalizeSecretKey(hex, 'descriptor private key') };
    }
    return { publicKey: normalizePublicKey(hex, 'descriptor x-only public key') };
  }

  throw new Error('This prototype supports hex private keys and compressed public keys inside two-argument sp() descriptors.');
}

function splitDescriptorArgs(input: string): string[] {
  const args: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < input.length; i += 1) {
    const char = input[i];
    if (char === '(') depth += 1;
    if (char === ')') depth -= 1;
    if (char === ',' && depth === 0) {
      args.push(input.slice(start, i).trim());
      start = i + 1;
    }
  }
  args.push(input.slice(start).trim());
  return args.filter(Boolean);
}

function stripOrigin(input: string): string {
  const trimmed = input.trim();
  if (!trimmed.startsWith('[')) return trimmed;
  const end = trimmed.indexOf(']');
  if (end === -1) throw new Error('Malformed descriptor key origin.');
  return trimmed.slice(end + 1).trim();
}

function bech32mEncode(hrp: string, payload: Uint8Array): string {
  const data = [0, ...convertBits([...payload], 8, 5, true)];
  const checksum = createChecksum(hrp, data);
  return `${hrp}1${[...data, ...checksum].map((value) => CHARSET[value]).join('')}`;
}

function bech32mDecode(value: string): { hrp: string; payload: Uint8Array } {
  const text = value.trim();
  if (text !== text.toLowerCase() && text !== text.toUpperCase()) {
    throw new Error('Bech32m strings cannot mix uppercase and lowercase.');
  }
  const lower = text.toLowerCase();
  const separator = lower.lastIndexOf('1');
  if (separator <= 0 || separator + 7 > lower.length) throw new Error('Malformed Bech32m key expression.');
  const hrp = lower.slice(0, separator);
  const data = [...lower.slice(separator + 1)].map((char) => {
    const index = CHARSET.indexOf(char);
    if (index === -1) throw new Error('Invalid Bech32m character.');
    return index;
  });
  if (!verifyChecksum(hrp, data)) throw new Error('Invalid Bech32m checksum.');
  const withoutChecksum = data.slice(0, -6);
  if (withoutChecksum[0] !== 0) throw new Error('Unsupported silent payment key version.');
  return { hrp, payload: new Uint8Array(convertBits(withoutChecksum.slice(1), 5, 8, false)) };
}

function hrpExpand(hrp: string): number[] {
  const high = [...hrp].map((char) => char.charCodeAt(0) >> 5);
  const low = [...hrp].map((char) => char.charCodeAt(0) & 31);
  return [...high, 0, ...low];
}

function polymod(values: number[]): number {
  let chk = 1;
  for (const value of values) {
    const top = chk >> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ value;
    for (let i = 0; i < 5; i += 1) {
      if ((top >> i) & 1) chk ^= GEN[i];
    }
  }
  return chk;
}

function createChecksum(hrp: string, data: number[]): number[] {
  const values = [...hrpExpand(hrp), ...data, 0, 0, 0, 0, 0, 0];
  const mod = polymod(values) ^ BECH32M_CONST;
  const result: number[] = [];
  for (let p = 0; p < 6; p += 1) result.push((mod >> (5 * (5 - p))) & 31);
  return result;
}

function verifyChecksum(hrp: string, data: number[]): boolean {
  return polymod([...hrpExpand(hrp), ...data]) === BECH32M_CONST;
}

function convertBits(data: number[], fromBits: number, toBits: number, pad: boolean): number[] {
  let acc = 0;
  let bits = 0;
  const maxv = (1 << toBits) - 1;
  const maxAcc = (1 << (fromBits + toBits - 1)) - 1;
  const ret: number[] = [];
  for (const value of data) {
    if (value < 0 || value >> fromBits !== 0) throw new Error('Invalid data range in bit conversion.');
    acc = ((acc << fromBits) | value) & maxAcc;
    bits += fromBits;
    while (bits >= toBits) {
      bits -= toBits;
      ret.push((acc >> bits) & maxv);
    }
  }
  if (pad) {
    if (bits > 0) ret.push((acc << (toBits - bits)) & maxv);
  } else if (bits >= fromBits || ((acc << (toBits - bits)) & maxv)) {
    throw new Error('Invalid padding in Bech32m payload.');
  }
  return ret;
}

function pickField(fields: Map<string, string>, names: string[]): string | undefined {
  for (const name of names) {
    const normalized = normalizeFieldName(name);
    const value = fields.get(normalized);
    if (value) return value;
  }
  return undefined;
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, '').toLowerCase();
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function normalizeSecretKey(input: string, label: string): string {
  const hex = stripHex(input);
  if (!/^[0-9a-f]{64}$/.test(hex)) {
    throw new Error(`${label} must be a 32-byte hex string.`);
  }
  if (!secpUtils.isValidSecretKey(hexToBytes(hex))) {
    throw new Error(`${label} is not a valid secp256k1 secret key.`);
  }
  return hex;
}

function normalizePublicKey(input: string, label: string): string {
  const hex = stripHex(input);

  if (/^[0-9a-f]{64}$/.test(hex)) {
    return hex;
  }

  if (!/^(02|03)[0-9a-f]{64}$/.test(hex)) {
    throw new Error(`${label} must be a 32-byte x-only or 33-byte compressed hex public key.`);
  }
  if (!secpUtils.isValidPublicKey(hexToBytes(hex))) {
    throw new Error(`${label} is not a valid secp256k1 public key.`);
  }
  return hex;
}

function xOnlyFromPublicKey(publicKey: string): string {
  return publicKey.length === 64 ? publicKey : publicKey.slice(2);
}

function stripHex(input: string): string {
  return input.trim().toLowerCase().replace(/^0x/, '').replace(/\s+/g, '');
}

function concatBytes(...arrays: Uint8Array[]): Uint8Array {
  const length = arrays.reduce((sum, array) => sum + array.length, 0);
  const result = new Uint8Array(length);
  let offset = 0;
  for (const array of arrays) {
    result.set(array, offset);
    offset += array.length;
  }
  return result;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(hex)) {
    throw new Error('Invalid hex string.');
  }
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i += 1) {
    bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}
