import '../crypto/initBitcoinJs';
import * as bitcoin from 'bitcoinjs-lib';
import type { BlockProviderConfig, EsploraTransaction } from '../api/blockProviderApi';
import { fetchDemoTransactionsWithFallback } from '../api/blockProviderApi';
import type { WalletKeyMaterial } from '../keys/walletKeys';
import { importWalletKeyMaterial } from '../keys/walletKeys';
import type { WalletLabel, WalletSpendableUtxo, WalletTransaction } from '../state/types';

const now = new Date().toISOString();
const GENERATOR_COMPRESSED_PUBKEY = hexToBytes('0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798');

export interface DemoWalletData {
  labels: WalletLabel[];
  transactions: WalletTransaction[];
  spendableUtxos: WalletSpendableUtxo[];
  keyMaterial: WalletKeyMaterial;
  source: string;
}

export async function buildDemoWalletData(providers: BlockProviderConfig[], signal?: AbortSignal): Promise<DemoWalletData> {
  try {
    const result = await fetchDemoTransactionsWithFallback(providers, signal);
    const fallback = fallbackDemoData();
    const fetchedUtxos = demoUtxosFromBlockTransactions(result.transactions);
    const baseSpendableUtxos = fetchedUtxos.length ? fetchedUtxos : fallback.spendableUtxos;
    const mapped = mapEsploraTransactions(result.transactions);
    const rbfSample = createDemoRbfTransactionFromUtxo(baseSpendableUtxos[0]);
    const spendableUtxos = reserveDemoRbfInput(baseSpendableUtxos, rbfSample);
    return {
      ...fallback,
      spendableUtxos,
      transactions: [rbfSample, ...(mapped.length ? mapped : fallback.transactions.filter((tx) => tx.confirmations !== 0))],
      source: fetchedUtxos.length
        ? `${result.providerName} downloaded block sample: ${fetchedUtxos.length} fake wallet UTXO${fetchedUtxos.length === 1 ? '' : 's'}`
        : mapped.length
          ? `${result.providerName} recent block sample`
          : 'local demo data',
    };
  } catch {
    return fallbackDemoData();
  }
}

export function fallbackDemoData(): DemoWalletData {
  const labels = demoLabels();
  const demoChangeAddress = makeDemoChangeAddress();
  const fakeIncoming = '38'.repeat(32);
  const fakeSent = '91'.repeat(32);
  const spendableUtxos: WalletSpendableUtxo[] = [
      {
        id: 'demo-utxo-change-1',
        txid: '11'.repeat(32),
        vout: 0,
        valueSat: 820_000,
        scriptPubKey: bytesToHex(bitcoin.payments.p2wpkh({ pubkey: GENERATOR_COMPRESSED_PUBKEY, network: bitcoin.networks.bitcoin }).output!),
        confirmed: true,
        labelId: 1,
        address: demoChangeAddress,
        source: 'demo',
      },
      {
        id: 'demo-utxo-donations-1',
        txid: '22'.repeat(32),
        vout: 1,
        valueSat: 540_000,
        scriptPubKey: bytesToHex(bitcoin.payments.p2wpkh({ pubkey: GENERATOR_COMPRESSED_PUBKEY, network: bitcoin.networks.bitcoin }).output!),
        confirmed: true,
        labelId: 3,
        address: demoChangeAddress,
        source: 'demo',
      },
      {
        id: 'demo-utxo-invoice-1',
        txid: '33'.repeat(32),
        vout: 0,
        valueSat: 260_000,
        scriptPubKey: bytesToHex(bitcoin.payments.p2wpkh({ pubkey: GENERATOR_COMPRESSED_PUBKEY, network: bitcoin.networks.bitcoin }).output!),
        confirmed: true,
        labelId: 4,
        address: demoChangeAddress,
        source: 'demo',
      },
    ];
  const rbfSample = createDemoRbfTransactionFromUtxo(spendableUtxos[0]);

  return {
    labels,
    keyMaterial: demoWalletKeyMaterial(),
    spendableUtxos: reserveDemoRbfInput(spendableUtxos, rbfSample),
    transactions: [
      {
        id: fakeIncoming,
        txid: fakeIncoming,
        direction: 'received',
        amountSat: 540_000,
        dateTime: new Date(Date.now() - 86_400_000 * 3).toISOString(),
        labelId: 3,
        confirmations: 28,
        note: 'Demo received transaction grouped under /donations.',
        demo: true,
      },
      rbfSample,
      {
        id: fakeSent,
        txid: fakeSent,
        direction: 'sent',
        amountSat: 125_000,
        feeSat: 1_800,
        dateTime: new Date(Date.now() - 86_400_000).toISOString(),
        labelId: 4,
        confirmations: 6,
        note: 'Demo confirmed outgoing payment from /work/client-a.',
        demo: true,
      },
    ],
    source: 'local demo data',
  };
}

export function makeDemoChangeAddress(): string {
  return bitcoin.payments.p2wpkh({ pubkey: GENERATOR_COMPRESSED_PUBKEY, network: bitcoin.networks.bitcoin }).address!;
}

export function demoLabels(): WalletLabel[] {
  return [
    { id: 4, path: ['work', 'client-a'], createdAt: now, updatedAt: now },
    { id: 5, path: ['savings'], createdAt: now, updatedAt: now },
  ];
}

export function demoWalletKeyMaterial(): WalletKeyMaterial {
  return importWalletKeyMaterial(
    {
      privateScanKey: '0000000000000000000000000000000000000000000000000000000000000001',
      privateSpendKey: '0000000000000000000000000000000000000000000000000000000000000002',
    },
    'demo',
  );
}

function demoUtxosFromBlockTransactions(transactions: EsploraTransaction[]): WalletSpendableUtxo[] {
  const utxos: WalletSpendableUtxo[] = [];
  const labelCycle = [1, 3, 4, 5];
  for (const tx of transactions) {
    if (!tx.txid) continue;
    for (const [vout, output] of (tx.vout ?? []).entries()) {
      if (!output.scriptpubkey || !output.scriptpubkey_address || (output.value ?? 0) < 25_000) continue;
      const scriptType = output.scriptpubkey_type ?? '';
      if (!/v0_p2wpkh|v1_p2tr|v0_p2wsh/i.test(scriptType)) continue;
      const labelId = labelCycle[utxos.length % labelCycle.length];
      utxos.push({
        id: `demo-block-${tx.txid}-${vout}`,
        txid: tx.txid,
        vout,
        valueSat: Math.min(output.value ?? 0, 1_500_000),
        scriptPubKey: output.scriptpubkey,
        confirmed: true,
        labelId,
        address: output.scriptpubkey_address,
        source: 'demo',
      });
      if (utxos.length >= 6) return utxos;
    }
  }
  return utxos;
}


function reserveDemoRbfInput(utxos: WalletSpendableUtxo[], transaction: Pick<WalletTransaction, 'txid'>): WalletSpendableUtxo[] {
  if (utxos.length === 0) return utxos;
  return utxos.map((utxo, index) => index === 0 ? { ...utxo, reservedByTxid: transaction.txid } : utxo);
}

function createDemoRbfTransactionFromUtxo(utxo: WalletSpendableUtxo | undefined): WalletTransaction {
  const fallback = utxo ?? fallbackDemoData().spendableUtxos[0];
  const recipientSat = Math.max(1_000, Math.min(78_000, Math.floor(fallback.valueSat / 4)));
  const feeSat = Math.max(800, Math.min(2_500, Math.floor(fallback.valueSat / 100)));
  const changeSat = Math.max(546, fallback.valueSat - recipientSat - feeSat);
  const rawTxHex = createDemoRawTransaction(
    makeDemoChangeAddress(),
    'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh',
    recipientSat,
    changeSat,
    { txid: fallback.txid, vout: fallback.vout },
  );
  const txid = bitcoin.Transaction.fromHex(rawTxHex).getId();
  return {
    id: txid,
    txid,
    direction: 'sent',
    amountSat: recipientSat,
    feeSat,
    dateTime: new Date(Date.now() - 18 * 60 * 1000).toISOString(),
    labelId: fallback.labelId,
    confirmations: 0,
    rawTxHex,
    rbfChangeOutputIndex: 1,
    note: `Demo unconfirmed RBF transaction spending fake wallet UTXO ${fallback.txid}:${fallback.vout}. Increasing the fee subtracts from the change output.`,
    demo: true,
  };
}

function mapEsploraTransactions(transactions: EsploraTransaction[]): WalletTransaction[] {
  return transactions
    .filter((tx) => tx.txid)
    .slice(0, 4)
    .map((tx, index) => {
      const amountSat = Math.max(1, Math.min(2_500_000, Math.round((tx.vout ?? []).reduce((sum, output) => sum + (output.value ?? 0), 0) / Math.max(1, (tx.vout ?? []).length || 1))));
      return {
        id: `demo-${tx.txid}`,
        txid: tx.txid,
        direction: index % 2 === 0 ? 'received' : 'sent',
        amountSat,
        feeSat: index % 2 === 0 ? undefined : tx.fee ?? 1_000,
        dateTime: tx.status?.block_time ? new Date(tx.status.block_time * 1000).toISOString() : new Date(Date.now() - index * 3_600_000).toISOString(),
        labelId: index % 2 === 0 ? 3 : 4,
        confirmations: tx.status?.confirmed ? 3 + index : 0,
        note: 'Fetched from Esplora for demo display only; not treated as wallet-owned history.',
        demo: true,
      } satisfies WalletTransaction;
    });
}

function createDemoRawTransaction(
  changeAddress: string,
  recipientAddress: string,
  recipientSat: number,
  changeSat: number,
  input: { txid: string; vout: number } = { txid: '44'.repeat(32), vout: 0 },
): string {
  const tx = new bitcoin.Transaction();
  tx.version = 2;
  tx.addInput(hexToBytes(input.txid).reverse(), input.vout, 0xfffffffd);
  tx.addOutput(bitcoin.address.toOutputScript(recipientAddress, bitcoin.networks.bitcoin), BigInt(recipientSat));
  tx.addOutput(bitcoin.address.toOutputScript(changeAddress, bitcoin.networks.bitcoin), BigInt(changeSat));
  return tx.toHex();
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}
