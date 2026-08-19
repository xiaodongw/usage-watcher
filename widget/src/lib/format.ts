import type { Meter } from "../types/Meter";
import type { Provider } from "../types/Provider";
import type { Severity } from "../types/Severity";

/**
 * "4h 38m", "12m", "now". Deliberately two units at most — a widget this small
 * is read at a glance, and "4h 38m 12s" is noise that changes every second.
 */
export function countdown(iso: string | null, now: number): string | null {
  if (!iso) return null;
  const ms = new Date(iso).getTime() - now;
  if (ms <= 0) return "now";

  const mins = Math.floor(ms / 60_000);
  const days = Math.floor(mins / 1440);
  const hours = Math.floor((mins % 1440) / 60);
  const minutes = mins % 60;

  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return "<1m";
}

/** How long ago, for the "last updated" line. */
export function ago(iso: string, now: number): string {
  const secs = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function money(amount: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency,
      maximumFractionDigits: 2,
    }).format(amount);
  } catch {
    // An unrecognised currency code must not blank the tile.
    return `${amount.toFixed(2)} ${currency}`;
  }
}

/**
 * How full a meter is, 0–1, for the bar.
 *
 * A balance is inverted: a *full* bar means a lot used up, i.e. little left, so
 * the visual meaning of "the bar is filling" is the same for both kinds — you
 * are running out. Unlimited and unbounded balances have no meaningful
 * fraction, so they render as empty rather than pretending to a scale.
 */
export function fill(meter: Meter): number {
  switch (meter.kind.type) {
    case "window":
      return clamp(meter.kind.used_pct / 100);
    case "balance": {
      const { amount, of_total, unlimited } = meter.kind;
      if (unlimited || !of_total || of_total <= 0) return 0;
      return clamp(1 - amount / of_total);
    }
    case "spend":
      return 0;
  }
}

function clamp(n: number): number {
  return Math.min(1, Math.max(0, n));
}

/** The right-hand value: "19%", "$2.50", "unlimited". */
export function readout(meter: Meter): string {
  switch (meter.kind.type) {
    case "window":
      return `${Math.round(meter.kind.used_pct)}%`;
    case "balance":
      return meter.kind.unlimited ? "unlimited" : money(meter.kind.amount, meter.kind.currency);
    case "spend":
      return money(meter.kind.amount, meter.kind.currency);
  }
}

export function resetsAt(meter: Meter): string | null {
  return meter.kind.type === "window" ? meter.kind.resets_at : null;
}

const RANK: Record<Severity, number> = { normal: 0, warning: 1, critical: 2 };

/**
 * The single meter that best represents "how much trouble am I in" — used for
 * the tray badge, where there is room for exactly one number.
 *
 * Ties break towards the fuller meter, so of two warnings the more urgent wins.
 */
export function mostConstrained(providers: Provider[]): { provider: Provider; meter: Meter } | null {
  let best: { provider: Provider; meter: Meter } | null = null;
  for (const provider of providers) {
    for (const meter of provider.meters) {
      if (
        !best ||
        RANK[meter.severity] > RANK[best.meter.severity] ||
        (RANK[meter.severity] === RANK[best.meter.severity] && fill(meter) > fill(best.meter))
      ) {
        best = { provider, meter };
      }
    }
  }
  return best;
}
