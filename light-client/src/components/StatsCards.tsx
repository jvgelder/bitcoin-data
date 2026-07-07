import { useLightClientState } from '../state/LightClientProvider';
import { formatBytes, formatInteger } from '../utils/format';

export function StatsCards() {
  const { local } = useLightClientState();
  const cards = [
    ['Local height', formatInteger(local.lastHeight)],
    ['Blocks applied', formatInteger(local.totalBlocks)],
    ['Candidate outputs', formatInteger(local.totalCandidateOutputs)],
    ['Tweak entries', formatInteger(local.totalTweaks)],
    ['Spent IDs', formatInteger(local.totalSpentIds)],
    ['Core payload bytes', formatBytes(local.totalPayloadBytes)],
  ];

  return (
    <section className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-6">
      {cards.map(([label, value]) => (
        <div key={label} className="rounded-2xl border border-slate-800 bg-slate-900/75 p-4 shadow-xl shadow-slate-950/20">
          <div className="text-xs uppercase tracking-wide text-slate-500">{label}</div>
          <div className="mt-2 text-2xl font-semibold text-slate-50">{value}</div>
        </div>
      ))}
    </section>
  );
}
