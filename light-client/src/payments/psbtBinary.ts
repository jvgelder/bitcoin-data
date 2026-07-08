export const PSBT_MAGIC_HEX = '70736274ff';
export const PSBT_MAGIC_B64 = 'cHNidP8';
export const PSBT_MAGIC_BYTES = new Uint8Array([0x70, 0x73, 0x62, 0x74, 0xff]);

export type PsbtInput = string | Uint8Array;

export interface PsbtKeyValue {
  keyType: number;
  keyData: Uint8Array;
  value: Uint8Array;
}

export interface PsbtMapSummary {
  entries: PsbtKeyValue[];
  duplicateKeys: string[];
}

export interface PsbtEnvelope {
  version: 0 | 2 | number;
  isV2: boolean;
  hasUnsignedTx: boolean;
  inputCount?: number;
  outputCount?: number;
  globalMap: PsbtMapSummary;
  inputMaps: PsbtMapSummary[];
  outputMaps: PsbtMapSummary[];
}

export function psbtToBytes(input: PsbtInput): Uint8Array {
  if (input instanceof Uint8Array) return input;
  if (typeof input !== 'string') throw new Error('Input cannot be converted to bytes.');

  const text = input.trim();
  if (isHex(text)) return hexToBytes(text);
  if (isBase64(text)) return base64ToBytes(text);
  throw new Error('Input cannot be converted to bytes.');
}

export function isPsbtText(input: string): boolean {
  const trimmed = input.trim();
  if (trimmed.toLowerCase().startsWith(PSBT_MAGIC_HEX)) return true;
  if (trimmed.startsWith(PSBT_MAGIC_B64)) return true;
  try {
    return hasPsbtMagic(psbtToBytes(trimmed));
  } catch {
    return false;
  }
}

export function hasPsbtMagic(bytes: Uint8Array): boolean {
  if (bytes.length < PSBT_MAGIC_BYTES.length) return false;
  return PSBT_MAGIC_BYTES.every((byte, index) => bytes[index] === byte);
}

export function parsePsbtEnvelope(input: PsbtInput): PsbtEnvelope {
  const bytes = psbtToBytes(input);
  const reader = new ByteReader(bytes);
  const magic = reader.readBytes(PSBT_MAGIC_BYTES.length);
  if (!byteEquals(magic, PSBT_MAGIC_BYTES)) throw new Error('Not a PSBT: missing magic bytes.');

  const globalMap = readMap(reader);
  const hasUnsignedTx = globalMap.entries.some((entry) => entry.keyType === 0x00);
  const version = readGlobalVersion(globalMap);
  const inputCount = readCompactCountGlobal(globalMap, 0x04);
  const outputCount = readCompactCountGlobal(globalMap, 0x05);
  const inputMaps: PsbtMapSummary[] = [];
  const outputMaps: PsbtMapSummary[] = [];

  if (inputCount != null && outputCount != null) {
    for (let i = 0; i < inputCount; i += 1) inputMaps.push(readMap(reader));
    for (let i = 0; i < outputCount; i += 1) outputMaps.push(readMap(reader));
  }

  return {
    version,
    isV2: version === 2,
    hasUnsignedTx,
    inputCount,
    outputCount,
    globalMap,
    inputMaps,
    outputMaps,
  };
}

export function psbtToBase64(input: PsbtInput): string {
  return bytesToBase64(psbtToBytes(input));
}

export function psbtToHex(input: PsbtInput): string {
  return bytesToHex(psbtToBytes(input));
}

function readMap(reader: ByteReader): PsbtMapSummary {
  const entries: PsbtKeyValue[] = [];
  const seen = new Set<string>();
  const duplicateKeys: string[] = [];

  while (!reader.eof()) {
    const keyLen = reader.readCompactSize();
    if (keyLen === 0) break;
    const key = reader.readBytes(keyLen);
    if (key.length === 0) throw new Error('Malformed PSBT key with zero length inside map.');
    const valueLen = reader.readCompactSize();
    const value = reader.readBytes(valueLen);
    const keyHex = bytesToHex(key);
    if (seen.has(keyHex)) duplicateKeys.push(keyHex);
    seen.add(keyHex);
    entries.push({ keyType: key[0], keyData: key.slice(1), value });
  }

  return { entries, duplicateKeys };
}

function readGlobalVersion(map: PsbtMapSummary): number {
  const entry = map.entries.find((item) => item.keyType === 0xfb && item.keyData.length === 0);
  if (!entry) return 0;
  if (entry.value.length !== 4) throw new Error('Malformed PSBT_GLOBAL_VERSION length.');
  return readUint32LE(entry.value, 0);
}

function readCompactCountGlobal(map: PsbtMapSummary, type: number): number | undefined {
  const entry = map.entries.find((item) => item.keyType === type && item.keyData.length === 0);
  if (!entry) return undefined;
  const reader = new ByteReader(entry.value);
  const value = reader.readCompactSize();
  if (!reader.eof()) throw new Error(`Malformed compact-count global field 0x${type.toString(16)}.`);
  return value;
}

class ByteReader {
  private offset = 0;
  constructor(private readonly bytes: Uint8Array) {}

  eof(): boolean {
    return this.offset >= this.bytes.length;
  }

  readBytes(length: number): Uint8Array {
    if (length < 0 || this.offset + length > this.bytes.length) throw new Error('Truncated PSBT data.');
    const result = this.bytes.slice(this.offset, this.offset + length);
    this.offset += length;
    return result;
  }

  readCompactSize(): number {
    const first = this.readUint8();
    if (first < 0xfd) return first;
    if (first === 0xfd) return this.readUint16LE();
    if (first === 0xfe) return this.readUint32LE();
    const value = this.readUint64LE();
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error('PSBT compact-size value exceeds safe JavaScript integer range.');
    return Number(value);
  }

  private readUint8(): number {
    if (this.offset >= this.bytes.length) throw new Error('Truncated PSBT data.');
    return this.bytes[this.offset++];
  }

  private readUint16LE(): number {
    const value = this.bytes[this.offset] | (this.bytes[this.offset + 1] << 8);
    this.readBytes(2);
    return value;
  }

  private readUint32LE(): number {
    const value = readUint32LE(this.bytes, this.offset);
    this.readBytes(4);
    return value;
  }

  private readUint64LE(): bigint {
    const bytes = this.readBytes(8);
    let value = 0n;
    for (let i = 7; i >= 0; i -= 1) value = (value << 8n) + BigInt(bytes[i]);
    return value;
  }
}

function readUint32LE(bytes: Uint8Array, offset: number): number {
  if (offset + 4 > bytes.length) throw new Error('Truncated uint32.');
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function isHex(value: string): boolean {
  return value.length % 2 === 0 && /^[0-9a-f]+$/i.test(value);
}

function isBase64(value: string): boolean {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value) || value.length % 4 !== 0) return false;
  try {
    const roundTrip = bytesToBase64(base64ToBytes(value)).replace(/=+$/, '');
    return roundTrip === value.replace(/=+$/, '');
  } catch {
    return false;
  }
}

function byteEquals(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) if (a[i] !== b[i]) return false;
  return true;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.trim().toLowerCase();
  if (clean.length % 2 !== 0 || !/^[0-9a-f]*$/.test(clean)) throw new Error('Invalid hex string.');
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function base64ToBytes(value: string): Uint8Array {
  const binary = globalThis.atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.slice(i, i + chunkSize));
  }
  return globalThis.btoa(binary);
}
