import { useState } from 'react';
import { useLightClientState } from '../state/LightClientProvider';
import type { AppPage } from '../state/types';
import { Badge, Button, GhostButton } from './ui';

export function Header({ page, onNavigate }: { page: AppPage; onNavigate(page: AppPage): void }) {
  const state = useLightClientState();
  const [open, setOpen] = useState(false);
  const tone = state.status === 'error' ? 'red' : state.status === 'syncing' ? 'indigo' : 'slate';
  const canShowWalletMenu = state.wallet.setupComplete;

  return (
    <header className="mb-8 flex items-start justify-between gap-4">
      <div>
        <button type="button" onClick={() => onNavigate('wallet')} className="text-left">
          <p className="text-sm font-medium uppercase tracking-[0.2em] text-indigo-300">bitcoindata</p>
          <h1 className="mt-2 text-3xl font-semibold tracking-tight text-white lg:text-4xl">
            Silent Payments Wallet
          </h1>
        </button>
        <p className="mt-3 max-w-3xl text-sm leading-6 text-slate-400">
          Local wallet flow for Silent Payments light sync. Server requests stay block-level; wallet keys, labels, and history stay in the browser prototype.
        </p>
        <div className="mt-4 flex flex-wrap gap-2">
          <Badge tone={tone}>{state.status}</Badge>
          {state.manifest?.network ? <Badge tone="green">{state.manifest.network}</Badge> : null}
          {state.demo.enabled ? <Badge tone="yellow">demo mode</Badge> : null}
          {state.walletKey ? (
            <Badge tone={state.walletKey.mode === 'full-private' ? 'green' : 'indigo'}>
              {state.walletKey.mode === 'full-private' ? 'full private' : 'watch-only'}
            </Badge>
          ) : null}
        </div>
      </div>

      {canShowWalletMenu ? (
        <div className="relative shrink-0">
          <Button onClick={() => setOpen((value) => !value)} aria-label="Open menu">
            ☰
          </Button>
          {open ? (
            <div className="absolute right-0 z-20 mt-2 w-48 overflow-hidden rounded-2xl border border-slate-800 bg-slate-950 shadow-2xl shadow-black/40">
              {(['backup', 'labels', 'settings', 'import'] as AppPage[]).map((item) => (
                <GhostButton
                  key={item}
                  className={`w-full justify-start rounded-none ${page === item ? 'bg-slate-800 text-white' : ''}`}
                  onClick={() => {
                    setOpen(false);
                    onNavigate(item);
                  }}
                >
                  {title(item)}
                </GhostButton>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
    </header>
  );
}

function title(page: AppPage): string {
  return page[0].toUpperCase() + page.slice(1);
}
