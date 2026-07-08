import type { BlockSummary } from '../sync/lightBlock';
import type { ChainTip, HealthResponse, ManifestResponse } from '../api/types';
import type { WalletKeyMaterial } from '../keys/walletKeys';
import type {
  ClientEvent,
  ClientEventLevel,
  EndpointProvider,
  FiatCurrency,
  LightClientState,
  ProviderSettingsState,
  SyncProfileState,
  SyncSettingsState,
  WalletLabel,
  WalletOutputState,
  WalletSummaryState,
  WalletSpendableUtxo,
  WalletTransaction,
} from './types';

const MAX_RECENT_BLOCKS = 200;
const MAX_EVENTS = 120;

export const initialLocalState: LightClientState['local'] = {
  totalBlocks: 0,
  totalCandidateOutputs: 0,
  totalTweaks: 0,
  totalSpentIds: 0,
  totalPayloadBytes: 0,
  recentBlocks: [],
};

const now = new Date().toISOString();

export const defaultLabels: WalletLabel[] = [
  { id: 1, path: ['change'], createdAt: now, updatedAt: now },
  { id: 2, path: ['receiving'], createdAt: now, updatedAt: now },
  { id: 3, path: ['donations'], createdAt: now, updatedAt: now },
];

export const initialProviderSettings: ProviderSettingsState = {
  lightServers: [
    {
      id: 'local-light-server',
      name: 'Local light-server',
      kind: 'light-server',
      url: import.meta.env.VITE_LIGHT_SERVER_URL ?? 'http://127.0.0.1:3000',
      enabled: true,
      apiKeyPlacement: 'none',
    },
  ],
  blockProviders: [
    {
      id: 'blockstream-mainnet',
      name: 'Blockstream Esplora',
      kind: 'esplora-blocks',
      url: 'https://blockstream.info/api',
      enabled: true,
      apiKeyPlacement: 'none',
    },
    {
      id: 'mempool-mainnet',
      name: 'mempool.space',
      kind: 'esplora-blocks',
      url: 'https://mempool.space/api',
      enabled: true,
      apiKeyPlacement: 'none',
    },
  ],
  activeLightServerId: 'local-light-server',
  activeBlockProviderId: 'blockstream-mainnet',
};

export const initialWalletState: WalletSummaryState = {
  setupComplete: false,
  balanceSat: 0,
  selectedFiat: 'EUR',
  fiatRatePerBtc: 0,
  importedHistory: false,
};

export const initialLightClientState: LightClientState = {
  serverUrl: initialProviderSettings.lightServers[0].url,
  status: 'idle',
  providers: initialProviderSettings,
  profile: {
    labels: 100,
    filterReuse: false,
    cutthrough: false,
  },
  settings: {
    startHeight: 0,
    rangeCount: 100,
    rescanHeight: 0,
  },
  local: initialLocalState,
  wallet: initialWalletState,
  labels: defaultLabels,
  transactions: [],
  demo: { enabled: false },
  walletOutputs: [],
  spendableUtxos: [],
  events: [],
};

export function setServerUrl(state: LightClientState, serverUrl: string): LightClientState {
  const providers = updateProviderList(state.providers, 'lightServers', {
    ...state.providers.lightServers.find((provider) => provider.id === state.providers.activeLightServerId),
    id: state.providers.activeLightServerId,
    name: state.providers.lightServers.find((provider) => provider.id === state.providers.activeLightServerId)?.name ?? 'Light server',
    kind: 'light-server',
    url: serverUrl,
    enabled: true,
    apiKeyPlacement: state.providers.lightServers.find((provider) => provider.id === state.providers.activeLightServerId)?.apiKeyPlacement ?? 'none',
  } as EndpointProvider);
  return { ...state, serverUrl, providers };
}

export function setProviderSettings(state: LightClientState, providers: ProviderSettingsState): LightClientState {
  const activeLight = getActiveProvider(providers.lightServers, providers.activeLightServerId);
  const activeBlock = getActiveProvider(providers.blockProviders, providers.activeBlockProviderId);
  return {
    ...state,
    serverUrl: activeLight?.url ?? state.serverUrl,
    providers: {
      ...providers,
      activeLightServerId: activeLight?.id ?? providers.lightServers[0]?.id ?? '',
      activeBlockProviderId: activeBlock?.id ?? providers.blockProviders[0]?.id ?? '',
    },
    events: addEvent(state.events, 'success', 'Saved provider settings'),
  };
}

export function upsertProvider(
  state: LightClientState,
  listName: 'lightServers' | 'blockProviders',
  provider: EndpointProvider,
): LightClientState {
  const providers = updateProviderList(state.providers, listName, provider);
  const selectedIdName = listName === 'lightServers' ? 'activeLightServerId' : 'activeBlockProviderId';
  const activeId = providers[selectedIdName] || provider.id;
  return setProviderSettings(state, { ...providers, [selectedIdName]: activeId });
}

export function removeProvider(
  state: LightClientState,
  listName: 'lightServers' | 'blockProviders',
  providerId: string,
): LightClientState {
  const currentList = state.providers[listName];
  if (currentList.length <= 1) {
    return { ...state, events: addEvent(state.events, 'error', 'Keep at least one provider in each list') };
  }
  const nextList = currentList.filter((provider) => provider.id !== providerId);
  const selectedIdName = listName === 'lightServers' ? 'activeLightServerId' : 'activeBlockProviderId';
  const nextSelected = state.providers[selectedIdName] === providerId ? nextList[0]?.id ?? '' : state.providers[selectedIdName];
  return setProviderSettings(state, { ...state.providers, [listName]: nextList, [selectedIdName]: nextSelected });
}

export function selectProvider(
  state: LightClientState,
  listName: 'lightServers' | 'blockProviders',
  providerId: string,
): LightClientState {
  const selectedIdName = listName === 'lightServers' ? 'activeLightServerId' : 'activeBlockProviderId';
  return setProviderSettings(state, { ...state.providers, [selectedIdName]: providerId });
}

export function enabledProviders(providers: EndpointProvider[], activeId: string): EndpointProvider[] {
  const enabled = providers.filter((provider) => provider.enabled);
  const active = enabled.find((provider) => provider.id === activeId);
  const rest = enabled.filter((provider) => provider.id !== activeId);
  return active ? [active, ...rest] : enabled;
}

export function setProfile(state: LightClientState, profile: Partial<SyncProfileState>): LightClientState {
  return { ...state, profile: { ...state.profile, ...profile } };
}

export function setSettings(state: LightClientState, settings: Partial<SyncSettingsState>): LightClientState {
  return { ...state, settings: { ...state.settings, ...settings } };
}

export function setWalletSettings(
  state: LightClientState,
  settings: Partial<Pick<WalletSummaryState, 'selectedFiat' | 'fiatRatePerBtc'>>,
): LightClientState {
  return { ...state, wallet: { ...state.wallet, ...settings } };
}

export function startConnecting(state: LightClientState): LightClientState {
  return {
    ...state,
    status: 'connecting',
    error: undefined,
    events: addEvent(state.events, 'info', 'Connecting to light server'),
  };
}

export function connectSucceeded(
  state: LightClientState,
  health: HealthResponse,
  manifest: ManifestResponse,
  tip: ChainTip,
): LightClientState {
  return {
    ...state,
    status: 'idle',
    health,
    manifest,
    tip,
    error: undefined,
    settings: {
      ...state.settings,
      rangeCount: Math.min(state.settings.rangeCount, manifest.max_range_count ?? state.settings.rangeCount),
    },
    events: addEvent(
      state.events,
      'success',
      `Connected to ${manifest.network ?? 'unknown'} light server at height ${tip.height}`,
    ),
  };
}

export function connectFailed(state: LightClientState, error: string): LightClientState {
  return {
    ...state,
    status: 'error',
    error,
    events: addEvent(state.events, 'error', error),
  };
}

export function setTip(state: LightClientState, tip: ChainTip): LightClientState {
  return { ...state, tip };
}

export function startSync(state: LightClientState): LightClientState {
  return {
    ...state,
    status: 'syncing',
    error: undefined,
    events: addEvent(state.events, 'info', 'Sync started'),
  };
}

export function stopSync(state: LightClientState): LightClientState {
  return {
    ...state,
    status: 'stopped',
    events: addEvent(state.events, 'warning', 'Sync stopped'),
  };
}

export function failSync(state: LightClientState, error: string): LightClientState {
  return {
    ...state,
    status: 'error',
    error,
    events: addEvent(state.events, 'error', error),
  };
}

export function applyBlocks(
  state: LightClientState,
  blocks: BlockSummary[],
  rawSpentIdsByHeight: Array<{ height: number; spentIds: number[] }>,
): LightClientState {
  if (blocks.length === 0) {
    return state;
  }

  const lastBlock = blocks[blocks.length - 1];
  const spentSignals = new Map<number, number>();
  for (const entry of rawSpentIdsByHeight) {
    for (const uid of entry.spentIds) {
      spentSignals.set(uid, entry.height);
    }
  }

  const walletOutputs = state.walletOutputs.map((output) => {
    const signalHeight = spentSignals.get(output.uid);
    if (signalHeight == null || output.status === 'confirmed-spent') {
      return output;
    }
    return {
      ...output,
      status: 'spend-signal' as const,
      spendSignalHeight: output.spendSignalHeight ?? signalHeight,
      lastSeenSpentIdHeight: signalHeight,
    };
  });

  const signaledUids = walletOutputs
    .filter((output) => output.status === 'spend-signal' && spentSignals.has(output.uid))
    .map((output) => output.uid);

  return {
    ...state,
    status: 'idle',
    error: undefined,
    local: {
      lastHeight: lastBlock.height,
      lastBlockHash: lastBlock.blockHash,
      totalBlocks: state.local.totalBlocks + blocks.length,
      totalCandidateOutputs:
        state.local.totalCandidateOutputs + blocks.reduce((sum, block) => sum + block.outputCount, 0),
      totalTweaks: state.local.totalTweaks + blocks.reduce((sum, block) => sum + block.tweakCount, 0),
      totalSpentIds: state.local.totalSpentIds + blocks.reduce((sum, block) => sum + block.spentCount, 0),
      totalPayloadBytes:
        state.local.totalPayloadBytes +
        blocks.reduce((sum, block) => sum + block.truncatedHashBytes + block.spentIdBytes, 0),
      recentBlocks: [...state.local.recentBlocks, ...blocks].slice(-MAX_RECENT_BLOCKS),
    },
    walletOutputs,
    events: addManyEvents(
      state.events,
      [
        ['success', `Applied ${blocks.length} block${blocks.length === 1 ? '' : 's'} through height ${lastBlock.height}`],
        ...signaledUids.map((uid): [ClientEventLevel, string] => [
          'warning',
          `Spent-ID signal for UID ${uid}. Full spending block confirmation is still required.`,
        ]),
      ],
    ),
  };
}

export function setWalletKey(state: LightClientState, walletKey: WalletKeyMaterial): LightClientState {
  return {
    ...state,
    walletKey,
    wallet: {
      ...state.wallet,
      descriptor: walletKey.descriptor ?? state.wallet.descriptor,
    },
    events: addEvent(
      state.events,
      'success',
      walletKey.mode === 'full-private'
        ? 'Loaded private scan key and private spend key'
        : 'Loaded watch-only key material: private scan key and public spend key',
    ),
  };
}

export function clearWalletKey(state: LightClientState): LightClientState {
  return {
    ...state,
    walletKey: undefined,
    wallet: { ...state.wallet, setupComplete: false, setupKind: undefined, descriptor: undefined },
    events: addEvent(state.events, 'warning', 'Cleared wallet key material from memory'),
  };
}

export function completeWalletSetup(
  state: LightClientState,
  setup: Partial<WalletSummaryState> & { setupKind: NonNullable<WalletSummaryState['setupKind']> },
): LightClientState {
  return {
    ...state,
    wallet: {
      ...state.wallet,
      ...setup,
      setupComplete: true,
    },
    events: addEvent(state.events, 'success', 'Wallet setup complete'),
  };
}

export function importWalletHistory(
  state: LightClientState,
  imported: {
    wallet?: Partial<WalletSummaryState>;
    labels?: WalletLabel[];
    transactions?: WalletTransaction[];
    spendableUtxos?: WalletSpendableUtxo[];
    keyMaterial?: WalletKeyMaterial;
  },
): LightClientState {
  const transactions = imported.transactions ?? state.transactions;
  const spendableUtxos = imported.spendableUtxos ?? state.spendableUtxos;
  return {
    ...state,
    walletKey: imported.keyMaterial ?? state.walletKey,
    wallet: {
      ...state.wallet,
      ...imported.wallet,
      importedHistory: true,
      balanceSat: deriveOpenUtxoBalance(spendableUtxos),
    },
    labels: imported.labels?.length ? normalizeLabels(imported.labels) : state.labels,
    transactions,
    spendableUtxos,
    events: addEvent(state.events, 'success', 'Imported wallet backup/history'),
  };
}

export function addWalletTransaction(state: LightClientState, transaction: WalletTransaction): LightClientState {
  const transactions = [transaction, ...state.transactions.filter((tx) => tx.id !== transaction.id)];
  return {
    ...state,
    transactions,
    wallet: { ...state.wallet, balanceSat: deriveOpenUtxoBalance(state.spendableUtxos) },
    events: addEvent(state.events, 'success', `Recorded transaction ${transaction.txid}`),
  };
}

export function setSpendableUtxos(state: LightClientState, spendableUtxos: WalletSpendableUtxo[]): LightClientState {
  return {
    ...state,
    spendableUtxos,
    wallet: { ...state.wallet, balanceSat: deriveOpenUtxoBalance(spendableUtxos) },
  };
}


export function setDemoMode(
  state: LightClientState,
  enabled: boolean,
  demoData?: { transactions?: WalletTransaction[]; spendableUtxos?: WalletSpendableUtxo[]; labels?: WalletLabel[]; keyMaterial?: WalletKeyMaterial; source?: string },
): LightClientState {
  if (!enabled) {
    const transactions = state.transactions.filter((tx) => !tx.demo);
    const spendableUtxos = state.spendableUtxos.filter((utxo) => utxo.source !== 'demo');
    return {
      ...state,
      demo: { enabled: false },
      transactions,
      spendableUtxos,
      walletKey: state.walletKey?.source === 'demo' ? undefined : state.walletKey,
      wallet: state.demo.enabled && state.wallet.setupKind === 'history-import'
        ? { ...state.wallet, setupComplete: false, setupKind: undefined, importedHistory: false, balanceSat: deriveOpenUtxoBalance(spendableUtxos) }
        : { ...state.wallet, balanceSat: deriveOpenUtxoBalance(spendableUtxos) },
      events: addEvent(state.events, 'warning', 'Demo mode disabled'),
    };
  }

  const labels = demoData?.labels?.length ? normalizeLabels([...state.labels, ...demoData.labels]) : state.labels;
  const demoTransactions = demoData?.transactions ?? [];
  const nonDemoTransactions = state.transactions.filter((tx) => !tx.demo);
  const demoUtxos = demoData?.spendableUtxos ?? [];
  const nonDemoUtxos = state.spendableUtxos.filter((utxo) => utxo.source !== 'demo');
  const spendableUtxos = [...nonDemoUtxos, ...demoUtxos];

  return {
    ...state,
    demo: { enabled: true, loadedAt: new Date().toISOString(), source: demoData?.source ?? 'local demo data' },
    labels,
    walletKey: state.walletKey ?? demoData?.keyMaterial,
    transactions: [...demoTransactions, ...nonDemoTransactions],
    spendableUtxos,
    wallet: {
      ...state.wallet,
      setupComplete: true,
      setupKind: state.wallet.setupKind ?? 'history-import',
      importedHistory: true,
      balanceSat: deriveOpenUtxoBalance(spendableUtxos),
    },
    events: addEvent(state.events, 'success', `Demo mode enabled${demoData?.source ? ` using ${demoData.source}` : ''}`),
  };
}

export function setWalletTransactions(state: LightClientState, transactions: WalletTransaction[]): LightClientState {
  return {
    ...state,
    transactions,
    wallet: { ...state.wallet, balanceSat: deriveOpenUtxoBalance(state.spendableUtxos) },
  };
}

export function upsertLabel(state: LightClientState, label: Pick<WalletLabel, 'id' | 'path'>): LightClientState {
  const cleanPath = cleanLabelPath(label.path);
  if (cleanPath.length === 0) return state;
  const nowIso = new Date().toISOString();
  const existing = state.labels.find((entry) => entry.id === label.id);
  const nextLabel: WalletLabel = existing
    ? { ...existing, path: cleanPath, updatedAt: nowIso }
    : { id: label.id, path: cleanPath, createdAt: nowIso, updatedAt: nowIso };
  const labels = existing
    ? state.labels.map((entry) => (entry.id === label.id ? nextLabel : entry))
    : [...state.labels, nextLabel];
  return {
    ...state,
    labels: normalizeLabels(labels),
    events: addEvent(state.events, existing ? 'success' : 'success', `Saved label ${label.id}`),
  };
}

export function createNextLabel(state: LightClientState, path: string[]): LightClientState {
  const used = new Set(state.labels.map((label) => label.id));
  let nextId = 1;
  while (used.has(nextId) && nextId < 100) nextId += 1;
  if (nextId > 100) {
    return { ...state, events: addEvent(state.events, 'error', 'Maximum label id 100 reached') };
  }
  return upsertLabel(state, { id: nextId, path });
}

export function rescanFromHeight(state: LightClientState, height: number): LightClientState {
  const nextHeight = Math.max(0, Math.trunc(height));
  return {
    ...state,
    settings: {
      ...state.settings,
      startHeight: nextHeight,
      rescanHeight: nextHeight,
    },
    local: initialLocalState,
    walletOutputs: state.walletOutputs.map((output) => ({
      ...output,
      status: output.status === 'confirmed-spent' ? output.status : 'tracked',
      spendSignalHeight: undefined,
      lastSeenSpentIdHeight: undefined,
    })),
    events: addEvent(state.events, 'warning', `Prepared rescan from height ${nextHeight}`),
  };
}

export function trackUid(state: LightClientState, uid: number): LightClientState {
  if (state.walletOutputs.some((output) => output.uid === uid)) {
    return state;
  }

  const output: WalletOutputState = {
    uid,
    status: 'tracked',
    addedAt: new Date().toISOString(),
  };

  return {
    ...state,
    walletOutputs: [...state.walletOutputs, output].sort((a, b) => a.uid - b.uid),
    events: addEvent(state.events, 'info', `Tracking UID ${uid}`),
  };
}

export function untrackUid(state: LightClientState, uid: number): LightClientState {
  return {
    ...state,
    walletOutputs: state.walletOutputs.filter((output) => output.uid !== uid),
    events: addEvent(state.events, 'info', `Removed UID ${uid}`),
  };
}

export function markSpendConfirmed(state: LightClientState, uid: number): LightClientState {
  return {
    ...state,
    walletOutputs: state.walletOutputs.map((output) =>
      output.uid === uid ? { ...output, status: 'confirmed-spent' } : output,
    ),
    events: addEvent(state.events, 'success', `Confirmed spend for UID ${uid}`),
  };
}

export function resetLocalSync(state: LightClientState): LightClientState {
  return {
    ...state,
    local: initialLocalState,
    events: addEvent(state.events, 'warning', 'Local sync state reset'),
  };
}

export function appendClientEvent(
  state: LightClientState,
  level: ClientEventLevel,
  message: string,
): LightClientState {
  return { ...state, events: addEvent(state.events, level, message) };
}

export function clearEvents(state: LightClientState): LightClientState {
  return { ...state, events: [] };
}

export function setFiatDisplay(state: LightClientState, selectedFiat: FiatCurrency, fiatRatePerBtc: number): LightClientState {
  return {
    ...state,
    wallet: {
      ...state.wallet,
      selectedFiat,
      fiatRatePerBtc: Math.max(0, fiatRatePerBtc),
    },
  };
}


function updateProviderList(
  providers: ProviderSettingsState,
  listName: 'lightServers' | 'blockProviders',
  provider: EndpointProvider,
): ProviderSettingsState {
  const cleaned = cleanProvider(provider);
  const list = providers[listName];
  const exists = list.some((entry) => entry.id === cleaned.id);
  const nextList = exists ? list.map((entry) => (entry.id === cleaned.id ? cleaned : entry)) : [...list, cleaned];
  const selectedIdName = listName === 'lightServers' ? 'activeLightServerId' : 'activeBlockProviderId';
  const selectedId = providers[selectedIdName] || cleaned.id;
  return { ...providers, [listName]: nextList, [selectedIdName]: selectedId };
}

function cleanProvider(provider: EndpointProvider): EndpointProvider {
  return {
    ...provider,
    id: provider.id || `provider-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    name: provider.name.trim() || 'Unnamed provider',
    url: provider.url.trim().replace(/\/$/, ''),
    apiKey: provider.apiKey?.trim() || undefined,
    apiKeyName: provider.apiKeyName?.trim() || undefined,
  };
}

function getActiveProvider(providers: EndpointProvider[], activeId: string): EndpointProvider | undefined {
  return providers.find((provider) => provider.id === activeId) ?? providers[0];
}

function normalizeLabels(labels: WalletLabel[]): WalletLabel[] {
  const byId = new Map<number, WalletLabel>();
  for (const label of labels) {
    const clean = { ...label, path: cleanLabelPath(label.path) };
    if (clean.path.length > 0) byId.set(clean.id, clean);
  }
  return [...byId.values()].sort((a, b) => a.id - b.id);
}

function cleanLabelPath(path: string[]): string[] {
  return path
    .map((part) => part.trim())
    .filter(Boolean)
    .filter((part, index) => !(index === 0 && part.toLowerCase() === 'root'));
}

export function isOpenSpendableUtxo(utxo: WalletSpendableUtxo): boolean {
  return utxo.confirmed && !utxo.reservedByTxid;
}

export function deriveOpenUtxoBalance(utxos: WalletSpendableUtxo[]): number {
  return utxos.filter(isOpenSpendableUtxo).reduce((sum, utxo) => sum + utxo.valueSat, 0);
}

function addEvent(events: ClientEvent[], level: ClientEventLevel, message: string): ClientEvent[] {
  return addManyEvents(events, [[level, message]]);
}

function addManyEvents(events: ClientEvent[], additions: Array<[ClientEventLevel, string]>): ClientEvent[] {
  const next = additions.reduce<ClientEvent[]>((acc, [level, message]) => {
    acc.push({
      id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      level,
      message,
      timestamp: new Date().toISOString(),
    });
    return acc;
  }, [...events]);
  return next.slice(-MAX_EVENTS);
}
