import { useState } from 'react';
import { BlockProviderApi } from '../api/blockProviderApi';
import { LightSyncApi } from '../api/lightSyncApi';
import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import type { ApiKeyPlacement, EndpointProvider } from '../state/types';
import { formatInteger, shortHash } from '../utils/format';
import { Button, Card, Field, Input, PrimaryButton, Select, Toggle } from './ui';

type ProviderListName = 'lightServers' | 'blockProviders';
type ProviderTestKind = 'light-status' | 'light-block' | 'block-hash' | 'raw-block';

type TestResult = {
  status: 'running' | 'success' | 'error';
  message: string;
};

export function ServerPanel() {
  const state = useLightClientState();
  const actions = useLightClientActions();
  const busy = state.status === 'connecting' || state.status === 'syncing';
  const [testHeight, setTestHeight] = useState(String(state.tip?.height ?? state.settings.startHeight ?? 0));
  const [testResults, setTestResults] = useState<Record<string, TestResult>>({});

  function setResult(providerId: string, kind: ProviderTestKind, result: TestResult) {
    setTestResults((current) => ({ ...current, [`${providerId}:${kind}`]: result }));
  }

  function getResult(providerId: string, kind: ProviderTestKind): TestResult | undefined {
    return testResults[`${providerId}:${kind}`];
  }

  function parsedHeight(): number {
    const height = Number(testHeight);
    if (!Number.isSafeInteger(height) || height < 0) throw new Error('Enter a valid non-negative test height.');
    return height;
  }

  async function testProvider(provider: EndpointProvider, kind: ProviderTestKind) {
    setResult(provider.id, kind, { status: 'running', message: 'Testing…' });
    try {
      if (kind === 'light-status') {
        const api = new LightSyncApi(provider.url, providerAuth(provider));
        const [health, manifest, tip] = await Promise.all([api.getHealth(), api.getManifest(), api.getTip()]);
        setResult(provider.id, kind, {
          status: 'success',
          message: `OK: ${health.ok === false ? 'unhealthy' : 'healthy'}, ${manifest.network ?? 'unknown network'}, tip ${tip.height}`,
        });
        return;
      }

      if (kind === 'light-block') {
        const height = parsedHeight();
        const api = new LightSyncApi(provider.url, providerAuth(provider));
        const block = await api.getBlock(height, state.profile);
        setResult(provider.id, kind, {
          status: 'success',
          message: `OK: light block ${block.height}, ${shortHash(block.block_hash)}, ${formatInteger(block.tweaks?.length)} tweaks`,
        });
        return;
      }

      if (kind === 'block-hash') {
        const height = parsedHeight();
        const api = new BlockProviderApi(providerConfig(provider));
        const hash = await api.getBlockHashByHeight(height);
        setResult(provider.id, kind, { status: 'success', message: `OK: ${shortHash(hash)} for height ${height}` });
        return;
      }

      const height = parsedHeight();
      const api = new BlockProviderApi(providerConfig(provider));
      const raw = await api.getRawBlockByHeight(height);
      setResult(provider.id, kind, {
        status: 'success',
        message: `OK: ${formatInteger(raw.rawBlock.length)} raw bytes for ${shortHash(raw.blockHash)}`,
      });
    } catch (error) {
      setResult(provider.id, kind, { status: 'error', message: error instanceof Error ? error.message : String(error) });
    }
  }

  return (
    <div className="space-y-5">
      <Card title="Network providers" subtitle="Configure direct URLs for remote light servers and Esplora-compatible block providers. Enabled providers are tried in primary-first fallback order during normal use.">
        <div className="mb-5 max-w-sm">
          <Field label="Provider test height" hint="Used by the light-block and full-block download tests.">
            <Input type="number" min={0} value={testHeight} onChange={(event) => setTestHeight(event.target.value)} />
          </Field>
        </div>
        <div className="grid gap-4 lg:grid-cols-2">
          <ProviderList
            title="Light servers"
            listName="lightServers"
            providers={state.providers.lightServers}
            activeId={state.providers.activeLightServerId}
            busy={busy}
            onTest={(provider, kind) => void testProvider(provider, kind)}
            getResult={getResult}
          />
          <ProviderList
            title="Full-block providers"
            listName="blockProviders"
            providers={state.providers.blockProviders}
            activeId={state.providers.activeBlockProviderId}
            busy={busy}
            onTest={(provider, kind) => void testProvider(provider, kind)}
            getResult={getResult}
          />
        </div>
        <div className="mt-5 flex justify-end">
          <PrimaryButton onClick={() => void actions.connect()} disabled={busy}>
            Test enabled light-server fallback
          </PrimaryButton>
        </div>
      </Card>

      <Card title="Current light-server status" subtitle="This is the last successful status fetched through the enabled light-server fallback list.">
        <dl className="grid gap-3 text-sm sm:grid-cols-2 lg:grid-cols-4">
          <Info label="Network" value={state.manifest?.network ?? '—'} />
          <Info label="Tip height" value={formatInteger(state.tip?.height ?? state.manifest?.tip?.height)} />
          <Info label="Tip hash" value={shortHash(state.tip?.block_hash ?? state.manifest?.tip?.block_hash)} mono />
          <Info label="Max range" value={formatInteger(state.manifest?.max_range_count)} />
        </dl>

        {state.error && (
          <div className="mt-4 rounded-xl border border-rose-700/80 bg-rose-950/50 p-3 text-sm text-rose-100">
            {state.error}
          </div>
        )}
      </Card>
    </div>
  );
}

function ProviderList({
  title,
  listName,
  providers,
  activeId,
  busy,
  onTest,
  getResult,
}: {
  title: string;
  listName: ProviderListName;
  providers: EndpointProvider[];
  activeId: string;
  busy: boolean;
  onTest(provider: EndpointProvider, kind: ProviderTestKind): void;
  getResult(providerId: string, kind: ProviderTestKind): TestResult | undefined;
}) {
  const actions = useLightClientActions();

  function addProvider() {
    const id = `${listName}-${Date.now()}`;
    actions.upsertProvider(listName, {
      id,
      name: listName === 'lightServers' ? 'Custom light server' : 'Custom Esplora provider',
      kind: listName === 'lightServers' ? 'light-server' : 'esplora-blocks',
      url: listName === 'lightServers' ? 'https://light.example.com' : 'https://blockstream.info/api',
      enabled: true,
      apiKeyPlacement: 'none',
    });
    actions.selectProvider(listName, id);
  }

  return (
    <section className="rounded-2xl border border-slate-800 bg-slate-950/30 p-4">
      <div className="mb-4 flex items-center justify-between gap-3">
        <h3 className="font-semibold text-slate-100">{title}</h3>
        <Button disabled={busy} onClick={addProvider} className="px-3 py-1">Add</Button>
      </div>
      <div className="space-y-4">
        {providers.map((provider) => (
          <ProviderEditor
            key={provider.id}
            provider={provider}
            listName={listName}
            active={provider.id === activeId}
            busy={busy}
            onTest={onTest}
            getResult={(kind) => getResult(provider.id, kind)}
          />
        ))}
      </div>
    </section>
  );
}

function ProviderEditor({
  provider,
  listName,
  active,
  busy,
  onTest,
  getResult,
}: {
  provider: EndpointProvider;
  listName: ProviderListName;
  active: boolean;
  busy: boolean;
  onTest(provider: EndpointProvider, kind: ProviderTestKind): void;
  getResult(kind: ProviderTestKind): TestResult | undefined;
}) {
  const actions = useLightClientActions();
  const [draft, setDraft] = useState<EndpointProvider>(provider);

  function update<K extends keyof EndpointProvider>(key: K, value: EndpointProvider[K]) {
    setDraft((current) => ({ ...current, [key]: value }));
  }

  function save() {
    actions.upsertProvider(listName, draft);
  }

  return (
    <div className={`rounded-xl border p-3 ${active ? 'border-indigo-500 bg-indigo-950/20' : 'border-slate-800 bg-slate-950/50'}`}>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <label className="inline-flex items-center gap-2 text-sm text-slate-200">
          <input
            type="radio"
            checked={active}
            disabled={busy}
            onChange={() => actions.selectProvider(listName, provider.id)}
          />
          Primary
        </label>
        <Toggle checked={draft.enabled} onChange={(enabled) => update('enabled', enabled)} label="Enabled" />
      </div>
      <div className="grid gap-3">
        <Field label="Name">
          <Input value={draft.name} disabled={busy} onChange={(event) => update('name', event.target.value)} />
        </Field>
        <Field label="URL" hint={listName === 'blockProviders' ? 'Use an Esplora-compatible base URL, for example https://blockstream.info/api or https://mempool.space/api.' : 'Use the direct remote light-server URL, for example https://light.example.com or http://192.168.1.20:3000.'}>
          <Input value={draft.url} disabled={busy} onChange={(event) => update('url', event.target.value)} placeholder={listName === 'blockProviders' ? 'https://blockstream.info/api' : 'https://light.example.com'} />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="API key placement">
            <Select value={draft.apiKeyPlacement} disabled={busy} onChange={(event) => update('apiKeyPlacement', event.target.value as ApiKeyPlacement)}>
              <option value="none">None</option>
              <option value="header">Header</option>
              <option value="bearer">Bearer Authorization</option>
              <option value="query">Query parameter</option>
            </Select>
          </Field>
          <Field label="Header/query name" hint="Examples: x-api-key, api-key, api_key.">
            <Input value={draft.apiKeyName ?? ''} disabled={busy || draft.apiKeyPlacement === 'none' || draft.apiKeyPlacement === 'bearer'} onChange={(event) => update('apiKeyName', event.target.value)} />
          </Field>
        </div>
        <Field label="API key" hint="Kept only in browser memory in this prototype.">
          <Input type="password" value={draft.apiKey ?? ''} disabled={busy || draft.apiKeyPlacement === 'none'} onChange={(event) => update('apiKey', event.target.value)} placeholder={draft.apiKeyPlacement === 'none' ? 'Not used' : 'Provider key'} />
        </Field>
        <div className="flex flex-wrap justify-end gap-2">
          <Button disabled={busy} onClick={() => actions.removeProvider(listName, provider.id)}>Remove</Button>
          <PrimaryButton disabled={busy} onClick={save}>Save</PrimaryButton>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-950/40 p-3">
          <div className="mb-2 text-xs font-medium uppercase tracking-wide text-slate-500">Provider tests</div>
          {listName === 'lightServers' ? (
            <div className="flex flex-wrap gap-2">
              <Button disabled={busy} onClick={() => onTest(draft, 'light-status')}>Test health/tip</Button>
              <Button disabled={busy} onClick={() => onTest(draft, 'light-block')}>Test light download</Button>
            </div>
          ) : (
            <div className="flex flex-wrap gap-2">
              <Button disabled={busy} onClick={() => onTest(draft, 'block-hash')}>Test height lookup</Button>
              <Button disabled={busy} onClick={() => onTest(draft, 'raw-block')}>Test block download</Button>
            </div>
          )}
          <TestMessage result={getResult(listName === 'lightServers' ? 'light-status' : 'block-hash')} />
          <TestMessage result={getResult(listName === 'lightServers' ? 'light-block' : 'raw-block')} />
        </div>
      </div>
    </div>
  );
}

function TestMessage({ result }: { result?: TestResult }) {
  if (!result) return null;
  const classes = result.status === 'success'
    ? 'border-emerald-800 bg-emerald-950/40 text-emerald-100'
    : result.status === 'error'
      ? 'border-rose-800 bg-rose-950/40 text-rose-100'
      : 'border-sky-800 bg-sky-950/40 text-sky-100';
  return <div className={`mt-2 rounded-lg border p-2 text-xs ${classes}`}>{result.message}</div>;
}

function providerAuth(provider: EndpointProvider) {
  return {
    apiKey: provider.apiKey,
    apiKeyPlacement: provider.apiKeyPlacement,
    apiKeyName: provider.apiKeyName,
  };
}

function providerConfig(provider: EndpointProvider) {
  return {
    name: provider.name,
    url: provider.url,
    apiKey: provider.apiKey,
    apiKeyPlacement: provider.apiKeyPlacement,
    apiKeyName: provider.apiKeyName,
  };
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-950/40 p-3">
      <dt className="text-xs uppercase tracking-wide text-slate-500">{label}</dt>
      <dd className={`mt-1 truncate text-slate-200 ${mono ? 'font-mono' : ''}`}>{value}</dd>
    </div>
  );
}
