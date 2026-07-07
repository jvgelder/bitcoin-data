import { secp256k1 } from '@noble/curves/secp256k1.js';

const SECP256K1_ORDER = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;

export interface XOnlyPointAddTweakResult {
  parity: 0 | 1;
  xOnlyPubkey: Uint8Array;
}

export function isXOnlyPoint(p: Uint8Array): boolean {
  if (p.length !== 32) return false;
  try {
    const prefixed = new Uint8Array(33);
    prefixed[0] = 0x02;
    prefixed.set(p, 1);
    secp256k1.ProjectivePoint.fromHex(prefixed);
    return true;
  } catch {
    return false;
  }
}

export function xOnlyPointAddTweak(p: Uint8Array, tweak: Uint8Array): XOnlyPointAddTweakResult | null {
  if (p.length !== 32 || tweak.length !== 32) return null;

  try {
    const tweakNum = bytesToBigInt(tweak);
    if (tweakNum >= SECP256K1_ORDER) return null;

    const prefixed = new Uint8Array(33);
    prefixed[0] = 0x02;
    prefixed.set(p, 1);
    const point = secp256k1.ProjectivePoint.fromHex(prefixed);

    const result = tweakNum === 0n ? point : point.add(secp256k1.ProjectivePoint.BASE.multiply(tweakNum));
    if (result.equals(secp256k1.ProjectivePoint.ZERO)) return null;

    const affine = result.toAffine();
    const parity: 0 | 1 = (affine.y & 1n) === 0n ? 0 : 1;
    return { parity, xOnlyPubkey: bigIntToBytes(affine.x, 32) };
  } catch {
    return null;
  }
}

function bytesToBigInt(bytes: Uint8Array): bigint {
  let result = 0n;
  for (const byte of bytes) result = (result << 8n) + BigInt(byte);
  return result;
}

function bigIntToBytes(num: bigint, length: number): Uint8Array {
  const bytes = new Uint8Array(length);
  for (let i = length - 1; i >= 0; i -= 1) {
    bytes[i] = Number(num & 0xffn);
    num >>= 8n;
  }
  return bytes;
}

export const ecc = { isXOnlyPoint, xOnlyPointAddTweak };
export default ecc;
