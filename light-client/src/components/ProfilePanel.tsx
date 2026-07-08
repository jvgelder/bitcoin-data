import { useLightClientActions, useLightClientState } from '../state/LightClientProvider';
import { Card, Field, Input, Select, Toggle } from './ui';

export function ProfilePanel() {
  const { profile, status } = useLightClientState();
  const actions = useLightClientActions();
  const busy = status === 'syncing' || status === 'connecting';

  return (
    <Card title="Request profile" subtitle="These are public bandwidth/filter parameters, not wallet labels or addresses.">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Label budget">
          <Select
            value={profile.labels}
            disabled={busy}
            onChange={(event) => actions.setProfile({ labels: Number(event.target.value) })}
          >
            <option value={2}>2 labels</option>
            <option value={100}>100 labels</option>
          </Select>
        </Field>
        <Field label="Response byte cap" hint="Optional max_bytes query parameter.">
          <Input
            type="number"
            min={1}
            value={profile.maxBytes ?? ''}
            disabled={busy}
            placeholder="server default"
            onChange={(event) =>
              actions.setProfile({ maxBytes: event.target.value ? Number(event.target.value) : undefined })
            }
          />
        </Field>
        <Toggle
          label="Filter reuse"
          checked={profile.filterReuse}
          onChange={(filterReuse) => actions.setProfile({ filterReuse })}
        />
        <Toggle
          label="Cut-through"
          checked={profile.cutthrough}
          onChange={(cutthrough) => actions.setProfile({ cutthrough })}
        />
        {profile.cutthrough && (
          <>
            <Field label="Cut-through start">
              <Input
                type="number"
                min={0}
                value={profile.cutthroughStart ?? ''}
                disabled={busy}
                placeholder="range start"
                onChange={(event) =>
                  actions.setProfile({
                    cutthroughStart: event.target.value ? Number(event.target.value) : undefined,
                  })
                }
              />
            </Field>
            <Field label="Cut-through tip">
              <Input
                type="number"
                min={0}
                value={profile.cutthroughTip ?? ''}
                disabled={busy}
                placeholder="server tip"
                onChange={(event) =>
                  actions.setProfile({ cutthroughTip: event.target.value ? Number(event.target.value) : undefined })
                }
              />
            </Field>
          </>
        )}
      </div>
    </Card>
  );
}
