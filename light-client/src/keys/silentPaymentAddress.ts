import * as tinysecp from 'tiny-secp256k1';
import type { WalletKeyMaterial } from './walletKeys';
import { bytesToHex, hexToBytes } from './walletKeys';

const BECH32M_CONST = 0x2bc830a3;
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
const SECP256K1_N = BigInt('0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141');

export async function silentPaymentAddressForLabel(
  walletKey: WalletKeyMaterial,
  labelId: number,
  network?: string,
): Promise<string> {
  const hrp = network && network !== 'bitcoin' && network !== 'mainnet' ? 'tsp' : 'sp';
  const scanPublicKey = compressedPublicKey(walletKey.scanPublicKey, walletKey.scanPublicKeyXOnly, 'scan public key');
  const spendPublicKey = compressedPublicKey(walletKey.spendPublicKey, walletKey.spendPublicKeyXOnly, 'spend public key');
  const labelNumber = walletLabelIdToBip352Label(labelId);
  const labeledSpendPublicKey = labelNumber > 0
    ? await addBip352LabelTweak(spendPublicKey, walletKey.privateScanKey, labelNumber)
    : spendPublicKey;
  return bech32mEncode(hrp, [0, ...convertBits([...hexToBytes(scanPublicKey), ...hexToBytes(labeledSpendPublicKey)], 8, 5, true)]);
}

export function walletLabelIdToBip352Label(labelId: number): number {
  return labelId <= 1 ? 0 : labelId - 1;
}

async function addBip352LabelTweak(spendPublicKey: string, privateScanKey: string, labelNumber: number): Promise<string> {
  const tagHash = await sha256(new TextEncoder().encode('BIP0352/Label'));
  const data = concatBytes(tagHash, tagHash, hexToBytes(privateScanKey), ser32(labelNumber));
  const hash = await sha256(data);
  const tweak = bytesToBigInt(hash) % SECP256K1_N;
  if (tweak === 0n) return spendPublicKey;

  const tweakBytes = bigIntToBytes(tweak, 32);
  const labeled = tinysecp.pointAddScalar(hexToBytes(spendPublicKey), tweakBytes, true);
  if (!labeled) throw new Error('Unable to apply Silent Payment label tweak to spend public key.');
  return bytesToHex(labeled);
}

function compressedPublicKey(compressed?: string, xonly?: string, label = 'public key'): string {
  if (compressed && /^(02|03)[0-9a-f]{64}$/i.test(compressed)) return compressed.toLowerCase();
  if (xonly && /^[0-9a-f]{64}$/i.test(xonly)) return `02${xonly.toLowerCase()}`;
  if (compressed && /^[0-9a-f]{64}$/i.test(compressed)) return `02${compressed.toLowerCase()}`;
  throw new Error(`Missing ${label}.`);
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

function ser32(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, false);
  return bytes;
}

function bytesToBigInt(bytes: Uint8Array): bigint {
  return BigInt(`0x${bytesToHex(bytes)}`);
}

function bigIntToBytes(value: bigint, length: number): Uint8Array {
  const bytes = new Uint8Array(length);
  let remaining = value;
  for (let i = length - 1; i >= 0; i -= 1) {
    bytes[i] = Number(remaining & 0xffn);
    remaining >>= 8n;
  }
  return bytes;
}

function bech32mEncode(hrp: string, data: number[]): string {
  const checksum = createChecksum(hrp, data);
  return `${hrp}1${[...data, ...checksum].map((value) => CHARSET[value]).join('')}`;
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
