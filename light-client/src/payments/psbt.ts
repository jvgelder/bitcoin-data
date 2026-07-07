import '../crypto/initBitcoinJs';
import * as bitcoin from 'bitcoinjs-lib';
import { inspectBip375Psbt } from './bip375';
import { isPsbtText, psbtToBase64, psbtToBytes } from './psbtBinary';
import type { PendingPaymentRequest, WalletSpendableUtxo } from '../state/types';

export interface ExpectedTxOutput {
  address?: string;
  amountSat: number;
  scriptPubKey: string;
}

export interface UnsignedPsbtPlan {
  psbtBase64: string;
  expectedOutputs: ExpectedTxOutput[];
  selectedUtxos: WalletSpendableUtxo[];
  feeSat: number;
  psbtVersion: 0 | 2;
  containsSilentPaymentOutputs: boolean;
  changeSat: number;
}

interface CreatePsbtArgs {
  payment: PendingPaymentRequest;
  utxos: WalletSpendableUtxo[];
  changeAddress?: string;
  feeRateSatVb: number;
  networkName?: string;
}

export function createUnsignedPsbtPlan(args: CreatePsbtArgs): UnsignedPsbtPlan {
  const amountSat = args.payment.amountSat;
  if (amountSat == null || amountSat <= 0) throw new Error('Payment request must contain an amount before creating a PSBT.');
  if (args.utxos.length === 0) {
    throw new Error('No confirmed spendable UTXOs are available yet. Sync/import confirmed UTXOs before creating PSBTs.');
  }

  const network = bitcoinNetwork(args.networkName);
  if (isSilentPaymentAddress(args.payment.address)) {
    throw new Error(
      'Sending to Silent Payment addresses requires BIP375 PSBTv2 output construction and DLEQ/share validation. The parser/validator scaffolding is present, but transaction construction is not enabled until confirmed input metadata and a BIP375-capable signer path are wired.',
    );
  }
  const recipientScript = bitcoin.address.toOutputScript(args.payment.address, network);
  const selected: WalletSpendableUtxo[] = [];
  let inputTotal = 0;
  let feeSat = 0;
  let changeSat = 0;

  for (const utxo of [...args.utxos].sort((a, b) => a.valueSat - b.valueSat)) {
    selected.push(utxo);
    inputTotal += utxo.valueSat;
    feeSat = estimateFeeSat(selected.length, args.changeAddress ? 2 : 1, args.feeRateSatVb);
    changeSat = inputTotal - amountSat - feeSat;
    if (changeSat >= 0) break;
  }

  if (changeSat < 0) throw new Error('Insufficient confirmed balance for amount plus estimated fee.');
  if (changeSat > 546 && !args.changeAddress) {
    throw new Error('A change address is required. The scanner/import layer must provide the wallet change output before this client can safely build a transaction.');
  }

  const psbt = new bitcoin.Psbt({ network });
  for (const utxo of selected) {
    psbt.addInput({
      hash: utxo.txid,
      index: utxo.vout,
      witnessUtxo: {
        script: hexToBytes(utxo.scriptPubKey),
        value: BigInt(utxo.valueSat),
      },
    });
  }
  psbt.addOutput({ address: args.payment.address, value: BigInt(amountSat) });
  if (changeSat > 546 && args.changeAddress) psbt.addOutput({ address: args.changeAddress, value: BigInt(changeSat) });

  return {
    psbtBase64: psbt.toBase64(),
    expectedOutputs: psbt.txOutputs.map((output) => ({
      address: safeAddressFromOutputScript(output.script, network),
      amountSat: Number(output.value),
      scriptPubKey: bytesToHex(output.script),
    })),
    selectedUtxos: selected,
    feeSat,
    psbtVersion: 0,
    containsSilentPaymentOutputs: false,
    changeSat: Math.max(0, changeSat),
  };
}


export function createUnsignedDemoRawTransaction(plan: UnsignedPsbtPlan): { rawTxHex: string; txid: string } {
  if (!plan.selectedUtxos.length) throw new Error('Demo send needs at least one selected UTXO.');
  const tx = new bitcoin.Transaction();
  tx.version = 2;
  for (const utxo of plan.selectedUtxos) {
    tx.addInput(hexToBytes(utxo.txid).reverse(), utxo.vout, 0xfffffffd);
  }
  for (const output of plan.expectedOutputs) {
    tx.addOutput(hexToBytes(output.scriptPubKey), BigInt(output.amountSat));
  }
  return { rawTxHex: tx.toHex(), txid: tx.getId() };
}

export function signPsbtLocallyOrThrow(_plan: UnsignedPsbtPlan): string {
  throw new Error(
    'Local automatic signing is not wired yet. Silent Payment spends need confirmed UTXO signing metadata from the scanner/import layer. Use the PSBT flow with a hardware/offline signer for now.',
  );
}

export function verifySignedPsbtOutputs(
  scanned: string,
  expectedOutputs: ExpectedTxOutput[],
  networkName?: string,
): { rawTxHex: string; txid: string; outputCount: number; psbtInspection?: string } {
  const network = bitcoinNetwork(networkName);
  const trimmed = scanned.trim();
  if (!trimmed) throw new Error('Signed PSBT scan was empty.');

  if (!isPsbtText(trimmed)) {
    throw new Error('Expected a signed PSBT in base64 or PSBT hex. Raw transaction hex is not accepted in this step.');
  }

  const inspection = safeInspectBip375(trimmed);
  const psbt = bitcoin.Psbt.fromBase64(psbtToBase64(trimmed), { network });
  assertOutputsMatch(
    psbt.txOutputs.map((output) => ({ amountSat: Number(output.value), scriptPubKey: bytesToHex(output.script) })),
    expectedOutputs,
  );
  const tx = psbt.extractTransaction();
  return { rawTxHex: tx.toHex(), txid: tx.getId(), outputCount: tx.outs.length, psbtInspection: inspection };
}

export function describePsbtInput(input: string): string {
  const bytes = psbtToBytes(input);
  const summary = inspectBip375Psbt(bytes);
  const parts = [
    `PSBTv${summary.psbtVersion}`,
    summary.inputCount == null ? undefined : `${summary.inputCount} input${summary.inputCount === 1 ? '' : 's'}`,
    summary.outputCount == null ? undefined : `${summary.outputCount} output${summary.outputCount === 1 ? '' : 's'}`,
    summary.silentPaymentOutputs ? `${summary.silentPaymentOutputs} Silent Payment output${summary.silentPaymentOutputs === 1 ? '' : 's'}` : undefined,
    summary.globalDleqProofs || summary.inputDleqProofs
      ? `${summary.globalDleqProofs + summary.inputDleqProofs} DLEQ proof${summary.globalDleqProofs + summary.inputDleqProofs === 1 ? '' : 's'}`
      : undefined,
  ].filter(Boolean);
  return parts.join(' · ') + (summary.warnings.length ? ` · warnings: ${summary.warnings.join(' ')}` : '');
}


function safeInspectBip375(input: string): string | undefined {
  try {
    return describePsbtInput(input);
  } catch {
    return undefined;
  }
}

function isSilentPaymentAddress(address: string): boolean {
  return /^(sp|tsp)1/i.test(address.trim());
}

function assertOutputsMatch(actual: ExpectedTxOutput[], expected: ExpectedTxOutput[]): void {
  if (actual.length !== expected.length) {
    throw new Error(`Signed transaction output count changed. Expected ${expected.length}, got ${actual.length}.`);
  }
  for (let i = 0; i < expected.length; i += 1) {
    const a = actual[i];
    const e = expected[i];
    if (a.amountSat !== e.amountSat || a.scriptPubKey.toLowerCase() !== e.scriptPubKey.toLowerCase()) {
      throw new Error(`Signed transaction output ${i + 1} changed. Refusing to broadcast.`);
    }
  }
}

function estimateFeeSat(inputCount: number, outputCount: number, feeRateSatVb: number): number {
  const vbytes = 10 + inputCount * 68 + outputCount * 43;
  return Math.max(1, Math.ceil(vbytes * Math.max(1, feeRateSatVb)));
}

function bitcoinNetwork(networkName?: string): bitcoin.Network {
  if (networkName === 'testnet' || networkName === 'signet') return bitcoin.networks.testnet;
  if (networkName === 'regtest') return bitcoin.networks.regtest;
  return bitcoin.networks.bitcoin;
}



export function createRbfReplacementFromRawTx(
  rawTxHex: string,
  changeOutputIndex: number,
  extraFeeSat: number,
): { rawTxHex: string; txid: string; feeDeltaSat: number } {
  if (!Number.isSafeInteger(extraFeeSat) || extraFeeSat <= 0) throw new Error('Enter a positive fee increase in sats.');
  const original = bitcoin.Transaction.fromHex(rawTxHex.trim());
  if (changeOutputIndex < 0 || changeOutputIndex >= original.outs.length) {
    throw new Error('RBF change output index is missing or invalid.');
  }
  const changeOutput = original.outs[changeOutputIndex];
  const currentValue = Number(changeOutput.value);
  if (currentValue - extraFeeSat < 546) throw new Error('Fee increase would reduce the change output below dust.');

  const replacement = new bitcoin.Transaction();
  replacement.version = Math.max(2, original.version);
  replacement.locktime = original.locktime;
  for (const input of original.ins) {
    replacement.addInput(input.hash, input.index, 0xfffffffd, input.script);
  }
  original.outs.forEach((output, index) => {
    const value = index === changeOutputIndex ? BigInt(currentValue - extraFeeSat) : output.value;
    replacement.addOutput(output.script, value);
  });
  return { rawTxHex: replacement.toHex(), txid: replacement.getId(), feeDeltaSat: extraFeeSat };
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.trim().toLowerCase();
  if (clean.length % 2 !== 0 || !/^[0-9a-f]*$/.test(clean)) throw new Error('Invalid hex string.');
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function safeAddressFromOutputScript(script: Uint8Array, network: bitcoin.Network): string | undefined {
  try {
    return bitcoin.address.fromOutputScript(script, network);
  } catch {
    return undefined;
  }
}
