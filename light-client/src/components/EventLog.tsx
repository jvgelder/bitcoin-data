import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { formatDateTime } from '../utils/format';
import { Button, Card } from './ui';

export function EventLog() {
  const { events } = useLightClientState();
  const actions = useLightClientActions();

  return (
    <Card title="Client log" subtitle="Validation failures, sync progress and spend signals.">
      <div className="mb-3 flex justify-end">
        <Button onClick={() => actions.clearEvents()} disabled={events.length === 0}>Clear</Button>
      </div>
      {events.length === 0 ? (
        <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-sm text-slate-500">
          No events yet.
        </div>
      ) : (
        <div className="max-h-96 space-y-2 overflow-y-auto pr-1">
          {[...events].reverse().map((event) => (
            <div key={event.id} className="rounded-xl border border-slate-800 bg-slate-950/50 p-3 text-sm">
              <div className="flex items-start justify-between gap-4">
                <p className={levelClass(event.level)}>{event.message}</p>
                <time className="shrink-0 text-xs text-slate-600">{formatDateTime(event.timestamp)}</time>
              </div>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function levelClass(level: string): string {
  switch (level) {
    case 'success':
      return 'text-emerald-200';
    case 'warning':
      return 'text-amber-200';
    case 'error':
      return 'text-rose-200';
    default:
      return 'text-slate-300';
  }
}
