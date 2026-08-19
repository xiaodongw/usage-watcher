/**
 * Rendering rules, kept deliberately identical to the widget's
 * `widget/src/lib/format.ts`.
 *
 * The two front ends read the same JSON and must not disagree about what it
 * means — a balance that fills one way in the panel and the other way in the
 * top bar would be worse than having only one of them.
 */

const RANK = { normal: 0, warning: 1, critical: 2 };

/** "4h 38m", "12m", "now". Two units at most; this is read at a glance. */
export function countdown(iso, nowMs) {
  if (!iso) return null;
  const ms = Date.parse(iso) - nowMs;
  if (Number.isNaN(ms)) return null;
  if (ms <= 0) return "now";

  const mins = Math.floor(ms / 60000);
  const days = Math.floor(mins / 1440);
  const hours = Math.floor((mins % 1440) / 60);
  const minutes = mins % 60;

  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return "<1m";
}

export function money(amount, currency) {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency,
      maximumFractionDigits: 2,
    }).format(amount);
  } catch {
    return `${amount.toFixed(2)} ${currency}`;
  }
}

/**
 * How full a meter is, 0–1.
 *
 * A balance is inverted: a full bar always means "you are running out",
 * whichever kind of meter it is. Unlimited and unbounded balances have no
 * meaningful fraction and render empty rather than inventing a scale.
 */
export function fill(meter) {
  const k = meter.kind;
  switch (k.type) {
    case "window":
      return clamp(k.used_pct / 100);
    case "balance":
      if (k.unlimited || !k.of_total || k.of_total <= 0) return 0;
      return clamp(1 - k.amount / k.of_total);
    default:
      return 0;
  }
}

function clamp(n) {
  return Math.min(1, Math.max(0, n));
}

/** The right-hand value: "19%", "$2.50", "unlimited". */
export function readout(meter) {
  const k = meter.kind;
  switch (k.type) {
    case "window":
      return `${Math.round(k.used_pct)}%`;
    case "balance":
      return k.unlimited ? "unlimited" : money(k.amount, k.currency);
    case "spend":
      return money(k.amount, k.currency);
    default:
      return "";
  }
}

export function resetsAt(meter) {
  return meter.kind.type === "window" ? meter.kind.resets_at : null;
}

/**
 * The single meter that best represents "how much trouble am I in".
 *
 * The top bar has room for one number, so this is what it shows. Ties break
 * towards the fuller meter, so of two warnings the more urgent wins.
 */
export function mostConstrained(providers) {
  let best = null;
  for (const provider of providers ?? []) {
    for (const meter of provider.meters ?? []) {
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

/**
 * What the top bar shows when there is nothing to headline.
 *
 * A provider can be perfectly healthy and still have no meters — an OpenRouter
 * free key, an opencode Zen key — so "no meters anywhere" is not the same as
 * "something is wrong", and the two must not look alike.
 */
export function severityOf(providers) {
  let worst = "normal";
  for (const p of providers ?? []) {
    for (const m of p.meters ?? []) {
      if (RANK[m.severity] > RANK[worst]) worst = m.severity;
    }
  }
  return worst;
}
