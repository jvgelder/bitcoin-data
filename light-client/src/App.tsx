import { useMemo, useState, type ReactNode } from 'react';
import { BackupPage } from './components/BackupPage';
import { Dashboard } from './components/Dashboard';
import { Header } from './components/Header';
import { ImportPage } from './components/ImportPage';
import { LabelEditor } from './components/LabelEditor';
import { OnboardingFlow } from './components/OnboardingFlow';
import { ReceivePage } from './components/ReceivePage';
import { SendPage } from './components/SendPage';
import { SettingsPage } from './components/SettingsPage';
import { TransactionDetail } from './components/TransactionDetail';
import { useLightClientState } from './state/LightClientProvider';
import type { AppPage } from './state/types';

export function App() {
  const state = useLightClientState();
  const [page, setPage] = useState<AppPage>('wallet');
  const [selectedTransactionId, setSelectedTransactionId] = useState<string>();
  const selectedTransaction = useMemo(
    () => state.transactions.find((tx) => tx.id === selectedTransactionId),
    [selectedTransactionId, state.transactions],
  );

  if (!state.wallet.setupComplete && page !== 'import') {
    return (
      <Shell page={page} onNavigate={setPage}>
        <OnboardingFlow onComplete={() => setPage('wallet')} onImport={() => setPage('import')} />
      </Shell>
    );
  }

  return (
    <Shell page={page} onNavigate={setPage}>
      {page === 'wallet' ? (
        <Dashboard
          onSend={() => setPage('send')}
          onReceive={() => setPage('receive')}
          onOpenTransaction={(id) => {
            setSelectedTransactionId(id);
            setPage('transaction');
          }}
        />
      ) : null}
      {page === 'send' ? <SendPage onDone={() => setPage('wallet')} /> : null}
      {page === 'receive' ? <ReceivePage /> : null}
      {page === 'backup' ? <BackupPage /> : null}
      {page === 'labels' ? <LabelEditor /> : null}
      {page === 'settings' ? <SettingsPage /> : null}
      {page === 'import' ? <ImportPage onImported={() => setPage('wallet')} /> : null}
      {page === 'transaction' ? (
        <TransactionDetail transaction={selectedTransaction} onBack={() => setPage('wallet')} />
      ) : null}
    </Shell>
  );
}

function Shell({ children, page, onNavigate }: { children: ReactNode; page: AppPage; onNavigate(page: AppPage): void }) {
  return (
    <main className="min-h-screen bg-[radial-gradient(circle_at_top_left,_rgba(79,70,229,0.22),_transparent_34rem),linear-gradient(180deg,_#020617_0%,_#0f172a_100%)] px-4 py-6 text-slate-100 sm:px-6 lg:px-8">
      <div className="mx-auto max-w-5xl">
        <Header page={page} onNavigate={onNavigate} />
        <div className="space-y-5">{children}</div>
      </div>
    </main>
  );
}
