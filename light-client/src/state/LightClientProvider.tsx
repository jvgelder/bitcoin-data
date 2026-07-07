import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
} from 'react';
import { LightSyncApi } from '../api/lightSyncApi';
import type { LightBlockRangeJson, LightClientQuery } from '../api/types';
import { assertRangeContinuity, summarizeBlock } from '../sync/lightBlock';
import type { WalletKeyMaterial } from '../keys/walletKeys';
import type {
  EndpointProvider,
  FiatCurrency,
  LightClientState,
  SyncProfileState,
  WalletLabel,
  WalletSummaryState,
  WalletSpendableUtxo,
  WalletTransaction,
} from './types';
import {
  addWalletTransaction as addWalletTransactionState,
  appendClientEvent,
  applyBlocks,
  clearEvents,
  clearWalletKey as clearWalletKeyState,
  completeWalletSetup as completeWalletSetupState,
  connectFailed,
  connectSucceeded,
  createNextLabel as createNextLabelState,
  enabledProviders,
  failSync,
  importWalletHistory as importWalletHistoryState,
  initialLightClientState,
  markSpendConfirmed as markSpendConfirmedState,
  resetLocalSync as resetLocalSyncState,
  removeProvider as removeProviderState,
  rescanFromHeight as rescanFromHeightState,
  selectProvider as selectProviderState,
  setFiatDisplay as setFiatDisplayState,
  setProfile as setProfileState,
  setServerUrl as setServerUrlState,
  setProviderSettings as setProviderSettingsState,
  setSettings as setSettingsState,
  setTip,
  setSpendableUtxos as setSpendableUtxosState,
  setWalletKey as setWalletKeyState,
  setWalletTransactions as setWalletTransactionsState,
  startConnecting,
  startSync,
  stopSync as stopSyncState,
  trackUid,
  untrackUid,
  upsertLabel as upsertLabelState,
  upsertProvider as upsertProviderState,
} from './lightClientState';

interface LightClientActions {
  setServerUrl(serverUrl: string): void;
  setProfile(profile: Partial<SyncProfileState>): void;
  setSettings(settings: Partial<LightClientState['settings']>): void;
  setProviderSettings(providers: LightClientState['providers']): void;
  upsertProvider(listName: 'lightServers' | 'blockProviders', provider: EndpointProvider): void;
  removeProvider(listName: 'lightServers' | 'blockProviders', providerId: string): void;
  selectProvider(listName: 'lightServers' | 'blockProviders', providerId: string): void;
  connect(): Promise<void>;
  syncRange(start: number, count: number): Promise<void>;
  syncToTip(): Promise<void>;
  stopSync(): void;
  addTrackedUid(uid: number): void;
  removeTrackedUid(uid: number): void;
  markSpendConfirmed(uid: number): void;
  resetLocalSync(): void;
  clearEvents(): void;
  setWalletKey(walletKey: WalletKeyMaterial): void;
  clearWalletKey(): void;
  completeWalletSetup(setup: Partial<WalletSummaryState> & { setupKind: NonNullable<WalletSummaryState['setupKind']> }): void;
  importWalletHistory(imported: {
    wallet?: Partial<WalletSummaryState>;
    labels?: WalletLabel[];
    transactions?: WalletTransaction[];
    spendableUtxos?: WalletSpendableUtxo[];
    keyMaterial?: WalletKeyMaterial;
  }): void;
  setWalletTransactions(transactions: WalletTransaction[]): void;
  addWalletTransaction(transaction: WalletTransaction): void;
  setSpendableUtxos(utxos: WalletSpendableUtxo[]): void;
  upsertLabel(label: Pick<WalletLabel, 'id' | 'path'>): void;
  createNextLabel(path: string[]): void;
  setFiatDisplay(currency: FiatCurrency, fiatRatePerBtc: number): void;
  rescanFromHeight(height: number): void;
}

const StateContext = createContext<LightClientState | undefined>(undefined);
const ActionsContext = createContext<LightClientActions | undefined>(undefined);

export function LightClientProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<LightClientState>(initialLightClientState);
  const stateRef = useRef(state);
  const abortRef = useRef<AbortController | null>(null);

  stateRef.current = state;

  const updateState = useCallback((updater: (current: LightClientState) => LightClientState) => {
    setState((current) => {
      const next = updater(current);
      stateRef.current = next;
      return next;
    });
  }, []);

  const stopSync = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    updateState(stopSyncState);
  }, [updateState]);

  const runWithAbort = useCallback(async <T,>(work: (signal: AbortSignal) => Promise<T>): Promise<T> => {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    try {
      return await work(controller.signal);
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
      }
    }
  }, []);

  const lightServerCandidates = useCallback((): EndpointProvider[] => {
    const current = stateRef.current;
    return enabledProviders(current.providers.lightServers, current.providers.activeLightServerId);
  }, []);

  const createLightApi = useCallback((provider: EndpointProvider): LightSyncApi => {
    return new LightSyncApi(provider.url, {
      apiKey: provider.apiKey,
      apiKeyPlacement: provider.apiKeyPlacement,
      apiKeyName: provider.apiKeyName,
    });
  }, []);

  const withLightServerFallback = useCallback(
    async <T,>(work: (api: LightSyncApi, provider: EndpointProvider) => Promise<T>): Promise<T> => {
      const providers = lightServerCandidates();
      if (providers.length === 0) throw new Error('Enable at least one light-server provider.');
      const errors: string[] = [];
      for (const provider of providers) {
        try {
          const result = await work(createLightApi(provider), provider);
          updateState((current) => selectProviderState(current, 'lightServers', provider.id));
          return result;
        } catch (error) {
          if (isAbortError(error)) throw error;
          errors.push(`${provider.name}: ${errorMessage(error)}`);
        }
      }
      throw new Error(`All light-server providers failed: ${errors.join('; ')}`);
    },
    [createLightApi, lightServerCandidates, updateState],
  );

  const connect = useCallback(async () => {
    updateState(startConnecting);
    try {
      await runWithAbort(async (signal) => {
        const { health, manifest, tip, provider } = await withLightServerFallback(async (api, provider) => {
          const [health, manifest, tip] = await Promise.all([
            api.getHealth(signal),
            api.getManifest(signal),
            api.getTip(signal),
          ]);
          return { health, manifest, tip, provider };
        });
        updateState((current) =>
          connectSucceeded(selectProviderState(current, 'lightServers', provider.id), health, manifest, tip),
        );
      });
    } catch (error) {
      if (isAbortError(error)) return;
      updateState((current) => connectFailed(current, errorMessage(error)));
    }
  }, [runWithAbort, updateState, withLightServerFallback]);

  const fetchAndApplyRange = useCallback(
    async (
      start: number,
      count: number,
      signal: AbortSignal,
      previousHashOverride?: string,
      cutthroughBoundary?: number,
    ): Promise<LightBlockRangeJson> => {
      const current = stateRef.current;
      const request = {
        start,
        count,
        ...profileToRangeQuery(current.profile, start, cutthroughBoundary),
      };
      const response = await withLightServerFallback((api) => api.getBlockRange(request, signal));
      const previousHash =
        previousHashOverride ??
        (current.local.lastHeight != null && start === current.local.lastHeight + 1
          ? current.local.lastBlockHash
          : undefined);
      assertRangeContinuity(response.blocks, start, previousHash);
      updateState((latest) =>
        applyBlocks(
          latest,
          response.blocks.map(summarizeBlock),
          response.blocks.map((block) => ({
            height: block.height,
            spentIds: block.decoded_spent_ids,
          })),
        ),
      );
      return response;
    },
    [updateState, withLightServerFallback],
  );

  const syncRange = useCallback(
    async (start: number, count: number) => {
      updateState(startSync);
      try {
        await runWithAbort(async (signal) => {
          await fetchAndApplyRange(start, count, signal);
        });
      } catch (error) {
        if (isAbortError(error)) return;
        updateState((current) => failSync(current, errorMessage(error)));
      }
    },
    [fetchAndApplyRange, runWithAbort, updateState],
  );

  const syncToTip = useCallback(async () => {
    updateState(startSync);
    try {
      await runWithAbort(async (signal) => {
        let madeProgress = false;
        let current = stateRef.current;
        let localHeight = current.local.lastHeight ?? current.settings.startHeight - 1;
        let previousHash = current.local.lastBlockHash;

        while (!signal.aborted) {
          const tip = await withLightServerFallback((api) => api.getTip(signal));
          updateState((latest) => setTip(latest, tip));

          current = stateRef.current;
          if (localHeight >= tip.height) {
            updateState((latest) =>
              appendClientEvent(
                latest,
                madeProgress ? 'success' : 'info',
                madeProgress ? `Synced to tip ${tip.height}` : `Already at tip ${tip.height}`,
              ),
            );
            return;
          }

          const nextStart = localHeight + 1;
          const remaining = tip.height - localHeight;
          const count = Math.max(1, Math.min(current.settings.rangeCount, remaining));
          const cutthroughBoundary =
            current.profile.cutthrough && current.profile.cutthroughStart == null
              ? current.settings.startHeight
              : current.profile.cutthroughStart;
          const response = await fetchAndApplyRange(nextStart, count, signal, previousHash, cutthroughBoundary);
          const lastBlock = response.blocks[response.blocks.length - 1];
          localHeight = lastBlock.height;
          previousHash = lastBlock.block_hash;
          madeProgress = true;
        }
      });
    } catch (error) {
      if (isAbortError(error)) return;
      updateState((current) => failSync(current, errorMessage(error)));
    }
  }, [fetchAndApplyRange, runWithAbort, updateState, withLightServerFallback]);

  const actions = useMemo<LightClientActions>(
    () => ({
      setServerUrl(serverUrl) {
        updateState((current) => setServerUrlState(current, serverUrl));
      },
      setProfile(profile) {
        updateState((current) => setProfileState(current, profile));
      },
      setSettings(settings) {
        updateState((current) => setSettingsState(current, settings));
      },
      setProviderSettings(providers) {
        updateState((current) => setProviderSettingsState(current, providers));
      },
      upsertProvider(listName, provider) {
        updateState((current) => upsertProviderState(current, listName, provider));
      },
      removeProvider(listName, providerId) {
        updateState((current) => removeProviderState(current, listName, providerId));
      },
      selectProvider(listName, providerId) {
        updateState((current) => selectProviderState(current, listName, providerId));
      },
      connect,
      syncRange,
      syncToTip,
      stopSync,
      addTrackedUid(uid) {
        updateState((current) => trackUid(current, uid));
      },
      removeTrackedUid(uid) {
        updateState((current) => untrackUid(current, uid));
      },
      markSpendConfirmed(uid) {
        updateState((current) => markSpendConfirmedState(current, uid));
      },
      resetLocalSync() {
        updateState(resetLocalSyncState);
      },
      clearEvents() {
        updateState(clearEvents);
      },
      setWalletKey(walletKey) {
        updateState((current) => setWalletKeyState(current, walletKey));
      },
      clearWalletKey() {
        updateState(clearWalletKeyState);
      },
      completeWalletSetup(setup) {
        updateState((current) => completeWalletSetupState(current, setup));
      },
      importWalletHistory(imported) {
        updateState((current) => importWalletHistoryState(current, imported));
      },
      setWalletTransactions(transactions) {
        updateState((current) => setWalletTransactionsState(current, transactions));
      },
      addWalletTransaction(transaction) {
        updateState((current) => addWalletTransactionState(current, transaction));
      },
      setSpendableUtxos(utxos) {
        updateState((current) => setSpendableUtxosState(current, utxos));
      },
      upsertLabel(label) {
        updateState((current) => upsertLabelState(current, label));
      },
      createNextLabel(path) {
        updateState((current) => createNextLabelState(current, path));
      },
      setFiatDisplay(currency, fiatRatePerBtc) {
        updateState((current) => setFiatDisplayState(current, currency, fiatRatePerBtc));
      },
      rescanFromHeight(height) {
        updateState((current) => rescanFromHeightState(current, height));
      },
    }),
    [connect, stopSync, syncRange, syncToTip, updateState],
  );

  return (
    <StateContext.Provider value={state}>
      <ActionsContext.Provider value={actions}>{children}</ActionsContext.Provider>
    </StateContext.Provider>
  );
}

export function useLightClientState(): LightClientState {
  const context = useContext(StateContext);
  if (!context) {
    throw new Error('useLightClientState must be used inside LightClientProvider');
  }
  return context;
}

export function useLightClientActions(): LightClientActions {
  const context = useContext(ActionsContext);
  if (!context) {
    throw new Error('useLightClientActions must be used inside LightClientProvider');
  }
  return context;
}

function profileToRangeQuery(
  profile: SyncProfileState,
  rangeStart: number,
  cutthroughBoundary?: number,
): LightClientQuery {
  const implicitCutthroughStart = profile.cutthrough ? cutthroughBoundary ?? rangeStart : undefined;
  return {
    labels: profile.labels,
    filterReuse: profile.filterReuse,
    cutthrough: profile.cutthrough && implicitCutthroughStart == null,
    cutthroughStart: profile.cutthroughStart ?? implicitCutthroughStart,
    cutthroughTip: profile.cutthroughTip,
    maxBytes: profile.maxBytes,
  };
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError';
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
