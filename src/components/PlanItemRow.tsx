import clsx from 'clsx';
import Text from './ui/Text';
import { TextVariants } from '../types/typography';
import { PlanItem } from '../types/plan';

function Badge({ children, tone }: { children: string; tone: 'info' | 'warning' | 'error' | 'success' }) {
  const toneClasses = {
    info: 'bg-blue-400/10 text-blue-400',
    warning: 'bg-yellow-400/10 text-yellow-400',
    error: 'bg-red-400/10 text-red-400',
    success: 'bg-green-400/10 text-green-400',
  }[tone];

  return <span className={clsx('px-2 py-0.5 rounded-md text-xs font-medium', toneClasses)}>{children}</span>;
}

function filename(path: string): string {
  return path.split('/').pop() ?? path;
}

// Same badge set (and the same precedence) as scan_cli's flag rendering in
// src-tauri/src/bin/scan_cli.rs, so the CLI dry-run and this UI never say
// different things about the same plan.
function badgesFor(item: PlanItem) {
  const badges: Array<{ tone: 'info' | 'warning' | 'error' | 'success'; text: string }> = [];
  if (item.no_op) badges.push({ tone: 'success', text: 'Already organized' });
  if (item.already_imported) badges.push({ tone: 'success', text: 'Already imported' });
  if (item.excluded) badges.push({ tone: 'info', text: 'Excluded' });
  if (item.needs_review) badges.push({ tone: 'warning', text: 'Needs review' });
  switch (item.conflict) {
    case 'destination_exists':
      badges.push({ tone: 'error', text: 'Conflict: destination exists' });
      break;
    case 'duplicate_at_destination':
      badges.push({ tone: 'info', text: 'Duplicate at destination' });
      break;
    case 'duplicate_in_plan':
      badges.push({ tone: 'error', text: 'Conflict: duplicate in plan' });
      break;
  }
  return badges;
}

export default function PlanItemRow({ item }: { item: PlanItem }) {
  const chosen = item.candidates[0];

  return (
    <div className="flex flex-col gap-1 py-3 border-b border-border-color last:border-0">
      <div className="flex items-center justify-between gap-4">
        <Text variant={TextVariants.body} color="primary" className="truncate">
          {filename(item.source_path)}
        </Text>
        {chosen && (
          <Text variant={TextVariants.small} className="shrink-0">
            {chosen.source} · {(chosen.confidence * 100).toFixed(0)}%
          </Text>
        )}
      </div>
      <Text variant={TextVariants.small} className="truncate">
        {item.destination_path ? `→ ${item.destination_path}` : '(unresolved — no destination)'}
      </Text>
      {badgesFor(item).length > 0 && (
        <div className="flex gap-1.5 flex-wrap mt-0.5">
          {badgesFor(item).map((b) => (
            <Badge key={b.text} tone={b.tone}>
              {b.text}
            </Badge>
          ))}
        </div>
      )}
    </div>
  );
}
