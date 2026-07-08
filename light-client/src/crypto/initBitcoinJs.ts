import { initEccLib } from 'bitcoinjs-lib';
import * as tinysecp from 'tiny-secp256k1';

let initialized = false;

export function initBitcoinJsEcc(): void {
  if (initialized) return;
  initEccLib(tinysecp);
  initialized = true;
}

initBitcoinJsEcc();
