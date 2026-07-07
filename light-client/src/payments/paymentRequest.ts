import '../crypto/initBitcoinJs';
import * as bitcoin from 'bitcoinjs-lib';
import { isPsbtText } from './psbtBinary';
import type { AddressScriptType, PendingPaymentRequest } from '../state/types';

const SATS_PER_BTC = 100_000_000;

export interface DetectedPaymentInput {
  kind: 'bitcoin-uri' | 'bip73-url' | 'payment-url' | 'address' | 'silent-payment' | 'psbt' | 'unknown';
  scriptType?: AddressScriptType;
  summary: string;
}

export async function resolveScannedPaymentRequest(rawText: string, signal?: AbortSignal): Promise<PendingPaymentRequest> {
  const value = rawText.trim();
  if (!value) throw new Error('Scanned QR code was empty.');

  if (isPsbtText(value)) {
    throw new Error('Scanned data is a PSBT. Use it in the signed-PSBT step, not as a payment request.');
  }

  if (isBitcoinUri(value)) {
    return parseBitcoinUri(value, 'bitcoin-uri', value);
  }

  if (isSilentPaymentAddress(value)) {
    return {
      address: value,
      source: 'silent-payment',
      original: value,
      detectedType: 'silent-payment',
    };
  }

  if (/^https?:\/\//i.test(value)) {
    const response = await fetch(value, {
      method: 'GET',
      headers: { Accept: 'text/uri-list' },
      signal,
    });
    if (!response.ok) throw new Error(`Payment request URL returned ${response.status} ${response.statusText}`);
    const contentType = response.headers.get('content-type') ?? '';
    const text = await response.text();
    const firstUri = firstUriListItem(text) ?? text.trim();
    if (!isBitcoinUri(firstUri)) {
      throw new Error(`Payment request did not return a bitcoin URI${contentType ? ` (${contentType})` : ''}.`);
    }
    return parseBitcoinUri(firstUri, 'bip73-url', value);
  }

  return parseManualAddressPayment(value, '', undefined, true);
}

export function parseBitcoinUri(uri: string, source: PendingPaymentRequest['source'] = 'bitcoin-uri', original = uri): PendingPaymentRequest {
  const text = uri.trim();
  if (!isBitcoinUri(text)) {
    throw new Error('Expected a bitcoin: payment URI.');
  }

  const withoutScheme = text.slice(text.indexOf(':') + 1);
  const [rawAddress, query = ''] = withoutScheme.split('?');
  const address = decodeURIComponent(rawAddress.trim());
  if (!address) throw new Error('Bitcoin URI did not contain an address.');
  const params = new URLSearchParams(query);
  const amount = params.get('amount');
  return {
    address,
    amountSat: amount ? btcToSats(amount) : undefined,
    label: params.get('label') ?? undefined,
    message: params.get('message') ?? undefined,
    source,
    original,
    detectedType: detectScriptType(address),
  };
}

export function parseManualAddressPayment(address: string, amountBtc: string, message?: string, allowMissingAmount = false): PendingPaymentRequest {
  const cleanAddress = address.trim();
  if (!cleanAddress) throw new Error('Enter a recipient address.');
  const detectedType = detectScriptType(cleanAddress);
  if (detectedType === 'unknown') throw new Error('Address is not valid for the active Bitcoin network or is not a recognized Silent Payment address.');
  const amountSat = amountBtc.trim() ? btcToSats(amountBtc.trim()) : undefined;
  if (!allowMissingAmount && (amountSat == null || amountSat <= 0)) throw new Error('Amount must be greater than zero.');
  return {
    address: cleanAddress,
    amountSat,
    message: message?.trim() || undefined,
    source: detectedType === 'silent-payment' ? 'silent-payment' : 'manual-address',
    original: amountBtc.trim() ? `${cleanAddress} ${amountBtc.trim()} BTC` : cleanAddress,
    detectedType,
  };
}

export function detectPaymentInput(value: string): DetectedPaymentInput {
  const trimmed = value.trim();
  if (!trimmed) return { kind: 'unknown', summary: 'Empty input' };
  if (isPsbtText(trimmed)) return { kind: 'psbt', summary: 'PSBT v1/v2 candidate' };
  if (isBitcoinUri(trimmed)) {
    try {
      const parsed = parseBitcoinUri(trimmed);
      return { kind: 'bitcoin-uri', scriptType: parsed.detectedType, summary: `bitcoin URI · ${scriptTypeLabel(parsed.detectedType)}` };
    } catch (error) {
      return { kind: 'unknown', summary: error instanceof Error ? error.message : String(error) };
    }
  }
  if (/^https?:\/\//i.test(trimmed)) return { kind: 'bip73-url', summary: 'BIP73/payment request URL' };
  if (isSilentPaymentAddress(trimmed)) return { kind: 'silent-payment', scriptType: 'silent-payment', summary: 'Silent Payment address' };
  const scriptType = detectScriptType(trimmed);
  if (scriptType !== 'unknown') return { kind: 'address', scriptType, summary: scriptTypeLabel(scriptType) };
  return { kind: 'unknown', summary: 'Unrecognized payment input' };
}

export function detectScriptType(address: string): AddressScriptType {
  const clean = address.trim();
  if (isSilentPaymentAddress(clean)) return 'silent-payment';
  try {
    const script = bitcoin.address.toOutputScript(clean, bitcoin.networks.bitcoin);
    if (script.length === 34 && script[0] === 0x51 && script[1] === 0x20) return 'p2tr';
    if (script.length === 22 && script[0] === 0x00 && script[1] === 0x14) return 'p2wpkh';
    if (script.length === 34 && script[0] === 0x00 && script[1] === 0x20) return 'p2wsh';
    if (script.length === 23 && script[0] === 0xa9 && script[1] === 0x14 && script[22] === 0x87) return 'p2sh';
    if (script.length === 25 && script[0] === 0x76 && script[1] === 0xa9 && script[2] === 0x14 && script[23] === 0x88 && script[24] === 0xac) return 'p2pkh';
  } catch {
    // fall through
  }
  return 'unknown';
}

export function scriptTypeLabel(type?: AddressScriptType): string {
  if (type === 'p2tr') return 'Taproot / p2tr';
  if (type === 'p2wpkh') return 'Native SegWit / p2wpkh';
  if (type === 'p2wsh') return 'Native SegWit script / p2wsh';
  if (type === 'p2sh') return 'Legacy wrapped script / p2sh';
  if (type === 'p2pkh') return 'Legacy / p2pkh';
  if (type === 'silent-payment') return 'Silent Payment address';
  return 'Unknown';
}

export function paymentRequestDisplaySource(source: PendingPaymentRequest['source']): string {
  if (source === 'bip73-url') return 'BIP73 URL';
  if (source === 'bitcoin-uri') return 'bitcoin URI';
  if (source === 'silent-payment') return 'Silent Payment';
  if (source === 'payment-url') return 'payment URL';
  return 'manual address';
}

function isBitcoinUri(value: string): boolean {
  return /^bitcoin:/i.test(value.trim());
}

function isSilentPaymentAddress(value: string): boolean {
  return /^(sp|tsp)1/i.test(value.trim());
}

function firstUriListItem(text: string): string | undefined {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line && !line.startsWith('#'));
}

function btcToSats(value: string): number {
  if (!/^\d+(\.\d{1,8})?$/.test(value)) throw new Error('Invalid BTC amount. Use up to 8 decimal places.');
  const [whole, fraction = ''] = value.split('.');
  return Number(whole) * SATS_PER_BTC + Number(fraction.padEnd(8, '0'));
}
