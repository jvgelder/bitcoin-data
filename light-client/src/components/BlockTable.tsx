import { useLightClientState } from '../state/LightClientProvider';
import { formatBytes, formatInteger, shortHash } from '../utils/format';
import { Card } from './ui';

export function BlockTable() {
  const { local } = useLightClientState();
  const blocks = [...local.recentBlocks].reverse();

  return (
    <Card title="Recent light blocks" subtitle="Candidate outputs are response-stream positions until a full block confirms a wallet match.">
      {blocks.length === 0 ? (
        <Empty text="No blocks applied yet." />
      ) : (
        <div className="overflow-x-auto">
          <table className="min-w-full divide-y divide-slate-800 text-left text-sm">
            <thead className="text-xs uppercase tracking-wide text-slate-500">
              <tr>
                <th className="px-3 py-2">Height</th>
                <th className="px-3 py-2">Block hash</th>
                <th className="px-3 py-2 text-right">Outputs</th>
                <th className="px-3 py-2 text-right">Tweaks</th>
                <th className="px-3 py-2 text-right">Hash bits</th>
                <th className="px-3 py-2 text-right">Hash bytes</th>
                <th className="px-3 py-2 text-right">Spent IDs</th>
                <th className="px-3 py-2 text-right">Spent bytes</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-900">
              {blocks.slice(0, 60).map((block) => (
                <tr key={block.height} className="text-slate-300 hover:bg-slate-900">
                  <td className="px-3 py-2 font-medium text-slate-100">{formatInteger(block.height)}</td>
                  <td className="px-3 py-2 font-mono text-xs text-slate-400">{shortHash(block.blockHash)}</td>
                  <td className="px-3 py-2 text-right">{formatInteger(block.outputCount)}</td>
                  <td className="px-3 py-2 text-right">{formatInteger(block.tweakCount)}</td>
                  <td className="px-3 py-2 text-right">{formatInteger(block.truncatedHashBits)}</td>
                  <td className="px-3 py-2 text-right">{formatBytes(block.truncatedHashBytes)}</td>
                  <td className="px-3 py-2 text-right">{formatInteger(block.spentCount)}</td>
                  <td className="px-3 py-2 text-right">{formatBytes(block.spentIdBytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

function Empty({ text }: { text: string }) {
  return <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-sm text-slate-500">{text}</div>;
}
