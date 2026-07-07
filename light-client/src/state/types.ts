import type { ChainTip, HealthResponse, ManifestResponse } from '../api/types';
import type { WalletKeyMaterial } from '../keys/walletKeys';
import type { BlockSummary } from '../sync/lightBlock';

export type WalletOutputStatus = 'tracked' | 'spend-signal' | 'confirmed-spent';
export type ClientEventLevel = 'info' | 'warning' | 'error' | 'success';
export type WalletFlowStep = 'onboarding' | 'cutthrough-choice' | 'offline-date' | 'wallet';
export type AppPage = 'wallet' | 'send' | 'receive' | 'backup' | 'labels' | 'settings' | 'import' | 'transaction';
export type WalletSetupKind = 'generated' | 'watch-only' | 'descriptor' | 'history-import';
export type FiatCurrency = 'EUR' | 'USD' | 'GBP' | 'CHF';
export type ProviderKind = 'light-server' | 'esplora-blocks';
export type ApiKeyPlacement = 'none' | 'header' | 'bearer' | 'query';
export type AddressScriptType = 'p2tr' | 'p2wpkh' | 'p2wsh' | 'p2sh' | 'p2pkh' | 'silent-payment' | 'unknown';

export interface SyncProfileState {
  labels: number;
  filterReuse: boolean;
  cutthrough: boolean;
  cutthroughStart?: number;
  cutthroughTip?: number;
  maxBytes?: number;
}

export interface SyncSettingsState {
  startHeight: number;
  rangeCount: number;
  rescanHeight: number;
}

export interface EndpointProvider {
  id: string;
  name: string;
  kind: ProviderKind;
  url: string;
  enabled: boolean;
  apiKey?: string;
  apiKeyPlacement: ApiKeyPlacement;
  apiKeyName?: string;
}

export interface ProviderSettingsState {
  lightServers: EndpointProvider[];
  blockProviders: EndpointProvider[];
  activeLightServerId: string;
  activeBlockProviderId: string;
}


export interface WalletSpendableUtxo {
  id: string;
  txid: string;
  vout: number;
  valueSat: number;
  scriptPubKey: string;
  confirmed: boolean;
  labelId: number;
  address?: string;
  source?: 'scanner' | 'history-import' | 'demo';
  /**
   * Set when the output is already used by a local/unconfirmed spend.
   * Reserved outputs are still wallet history, but they are not open/unused
   * balance and should not be selected for a new spend.
   */
  reservedByTxid?: string;
  derivedPrivateKey?: string;
}

export interface PendingPaymentRequest {
  address: string;
  amountSat?: number;
  label?: string;
  message?: string;
  source: 'bitcoin-uri' | 'bip73-url' | 'manual-address' | 'payment-url' | 'silent-payment';
  original: string;
  detectedType?: AddressScriptType;
}

export interface WalletOutputState {
  uid: number;
  status: WalletOutputStatus;
  addedAt: string;
  spendSignalHeight?: number;
  lastSeenSpentIdHeight?: number;
}

export interface ClientEvent {
  id: string;
  level: ClientEventLevel;
  message: string;
  timestamp: string;
}

export interface LocalSyncState {
  lastHeight?: number;
  lastBlockHash?: string;
  totalBlocks: number;
  totalCandidateOutputs: number;
  totalTweaks: number;
  totalSpentIds: number;
  totalPayloadBytes: number;
  recentBlocks: BlockSummary[];
}

export interface WalletLabel {
  id: number;
  path: string[];
  createdAt: string;
  updatedAt: string;
}

export interface WalletTransaction {
  id: string;
  txid: string;
  direction: 'sent' | 'received';
  amountSat: number;
  feeSat?: number;
  dateTime: string;
  labelId: number;
  confirmations?: number;
  note?: string;
  rawTxHex?: string;
  rbfChangeOutputIndex?: number;
}

export interface WalletSummaryState {
  setupComplete: boolean;
  setupKind?: WalletSetupKind;
  balanceSat: number;
  selectedFiat: FiatCurrency;
  fiatRatePerBtc: number;
  lastOnlineHeight?: number;
  lastOnlineAt?: string;
  importedHistory: boolean;
  descriptor?: string;
}

export interface LightClientState {
  serverUrl: string;
  status: 'idle' | 'connecting' | 'syncing' | 'error' | 'stopped';
  error?: string;
  health?: HealthResponse;
  manifest?: ManifestResponse;
  tip?: ChainTip;
  providers: ProviderSettingsState;
  profile: SyncProfileState;
  settings: SyncSettingsState;
  local: LocalSyncState;
  walletKey?: WalletKeyMaterial;
  wallet: WalletSummaryState;
  labels: WalletLabel[];
  transactions: WalletTransaction[];
  walletOutputs: WalletOutputState[];
  spendableUtxos: WalletSpendableUtxo[];
  events: ClientEvent[];
}
