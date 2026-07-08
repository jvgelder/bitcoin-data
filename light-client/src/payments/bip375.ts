import { bytesToHex, parsePsbtEnvelope, type PsbtEnvelope, type PsbtInput } from './psbtBinary';

export const PSBT_GLOBAL_SP_ECDH_SHARE = 0x07;
export const PSBT_GLOBAL_SP_DLEQ = 0x08;
export const PSBT_IN_SP_ECDH_SHARE = 0x1d;
export const PSBT_IN_SP_DLEQ = 0x1e;
export const PSBT_OUT_SP_V0_INFO = 0x09;
export const PSBT_OUT_SP_V0_LABEL = 0x0a;

export interface Bip375PsbtSummary {
  psbtVersion: number;
  inputCount?: number;
  outputCount?: number;
  globalEcdhShares: number;
  globalDleqProofs: number;
  inputEcdhShares: number;
  inputDleqProofs: number;
  silentPaymentOutputs: number;
  silentPaymentLabels: number;
  warnings: string[];
}

export function inspectBip375Psbt(input: PsbtInput): Bip375PsbtSummary {
  return summarizeBip375Envelope(parsePsbtEnvelope(input));
}

export function summarizeBip375Envelope(envelope: PsbtEnvelope): Bip375PsbtSummary {
  const warnings: string[] = [];
  const globalEcdhShares = envelope.globalMap.entries.filter((entry) => entry.keyType === PSBT_GLOBAL_SP_ECDH_SHARE).length;
  const globalDleqProofs = envelope.globalMap.entries.filter((entry) => entry.keyType === PSBT_GLOBAL_SP_DLEQ).length;
  let inputEcdhShares = 0;
  let inputDleqProofs = 0;
  let silentPaymentOutputs = 0;
  let silentPaymentLabels = 0;

  for (const map of envelope.inputMaps) {
    inputEcdhShares += map.entries.filter((entry) => entry.keyType === PSBT_IN_SP_ECDH_SHARE).length;
    inputDleqProofs += map.entries.filter((entry) => entry.keyType === PSBT_IN_SP_DLEQ).length;
  }

  for (const map of envelope.outputMaps) {
    const spInfoEntries = map.entries.filter((entry) => entry.keyType === PSBT_OUT_SP_V0_INFO);
    silentPaymentOutputs += spInfoEntries.length;
    silentPaymentLabels += map.entries.filter((entry) => entry.keyType === PSBT_OUT_SP_V0_LABEL).length;
    for (const entry of spInfoEntries) {
      if (entry.value.length !== 66) {
        warnings.push(`Silent Payment output info should contain 33-byte scan and 33-byte spend keys; saw ${entry.value.length} bytes.`);
      }
    }
  }

  for (const entry of envelope.globalMap.entries) {
    if ((entry.keyType === PSBT_GLOBAL_SP_ECDH_SHARE && entry.value.length !== 33) || (entry.keyType === PSBT_GLOBAL_SP_DLEQ && entry.value.length !== 64)) {
      warnings.push(`Malformed global Silent Payment field 0x${entry.keyType.toString(16)} for scan key ${bytesToHex(entry.keyData)}.`);
    }
  }
  for (const [index, map] of envelope.inputMaps.entries()) {
    for (const entry of map.entries) {
      if ((entry.keyType === PSBT_IN_SP_ECDH_SHARE && entry.value.length !== 33) || (entry.keyType === PSBT_IN_SP_DLEQ && entry.value.length !== 64)) {
        warnings.push(`Malformed input ${index} Silent Payment field 0x${entry.keyType.toString(16)}.`);
      }
    }
  }

  if (!envelope.isV2 && (globalEcdhShares || globalDleqProofs || inputEcdhShares || inputDleqProofs || silentPaymentOutputs)) {
    warnings.push('BIP375 Silent Payment fields are only valid for PSBTv2.');
  }
  if (envelope.globalMap.duplicateKeys.length || envelope.inputMaps.some((map) => map.duplicateKeys.length) || envelope.outputMaps.some((map) => map.duplicateKeys.length)) {
    warnings.push('Duplicate PSBT keys were detected.');
  }

  return {
    psbtVersion: envelope.version,
    inputCount: envelope.inputCount,
    outputCount: envelope.outputCount,
    globalEcdhShares,
    globalDleqProofs,
    inputEcdhShares,
    inputDleqProofs,
    silentPaymentOutputs,
    silentPaymentLabels,
    warnings,
  };
}
