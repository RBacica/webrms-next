// ── Forecast engine — pure functions, no I/O ───────────────────
// Every rule is explicit and unit-tested. Inputs come from the DB layer;
// all math here is deterministic given the inputs.

/// One day of sales: offset = days relative to today (0 = today, -1 = yesterday...).
#[derive(Debug, Clone, Copy)]
pub struct SaleDay {
    pub offset: i64,
    pub units: f64,
}

/// Per-product history fed into the engine.
#[derive(Debug, Clone, Default)]
pub struct ProductHistory {
    pub daily: Vec<SaleDay>,
    /// Day offsets (past and present) where a Special was active — excluded
    /// from the demand baseline so promos don't inflate the normal rate.
    pub promo_days: std::collections::HashSet<i64>,
    /// Future promo windows (start_offset, end_offset), inclusive.
    pub upcoming_promos: Vec<(i64, i64)>,
}

/// Everything the engine needs to size one order line.
#[derive(Debug, Clone)]
pub struct LineInput {
    pub on_hand: f64,
    pub on_order: f64,
    pub pack_size: f64, // PurchaseQty — 0 means "don't round" (treat as 1)
    pub min_qty: f64,   // 0 = unset
    pub max_qty: f64,   // 0 = unset
    pub no_order: bool,
    pub lead_days: i64,
    pub cover_days: i64, // lead + order cycle — stock to carry until next delivery
    pub shrink_pct: f64,
    pub promo_uplift_default: f64,
    pub dept_rate: f64, // fallback daily rate for new lines
    pub ignore_min_qty: bool, // skip Items.MinQty floor
    pub ignore_max_qty: bool, // skip Items.MaxQty cap
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LineResult {
    pub rate30: f64,
    pub rate90: f64,
    pub rate_forward: f64,
    pub cover_demand: f64,
    /// Actual units sold in the window (promo days excluded) — for display.
    pub units7: f64,
    pub units30: f64,
    pub units90: f64,
    /// Days until on-hand + on-order runs out at the forecast rate.
    pub sellout_days: Option<i64>,
    pub sale_days30: i64,
    pub forecast_to_arrival: f64,
    pub projected_stock: f64,
    pub suggested: f64,
    pub promo_uplift_used: f64,
    /// True when the line has NO sales history at all (90d). Suggestion is
    /// forced to 0 — the operator decides manually (new/replacement line).
    pub no_history: bool,
}

/// Non-promo daily rate over the trailing `window` days.
/// Returns (rate, sale_days). The denominator is CALENDAR days (window length
/// minus promo days), not sale-day entries — dividing by entries inflates
/// sparse sellers (2 units on one day ≠ 2 units/day).
pub fn baseline_rate(hist: &ProductHistory, window: i64) -> (f64, i64) {
    let mut units = 0.0f64;
    let mut promo_in_window = 0i64;
    let mut sale_days = 0i64;
    for sd in &hist.daily {
        if sd.offset >= -window && sd.offset <= 0 {
            if hist.promo_days.contains(&sd.offset) {
                continue;
            }
            units += sd.units;
            if sd.units > 0.0 {
                sale_days += 1;
            }
        }
    }
    for d in -window..=0 {
        if hist.promo_days.contains(&d) {
            promo_in_window += 1;
        }
    }
    let days = (window + 1) - promo_in_window; // calendar non-promo days
    if days <= 0 {
        (0.0, 0)
    } else {
        (units / days as f64, sale_days)
    }
}

/// Total units sold in the trailing `window` days (promo days excluded) — the
/// actual units behind `baseline_rate`, for display (07-Day/30-Day columns).
pub fn window_units(hist: &ProductHistory, window: i64) -> f64 {
    let mut units = 0.0f64;
    for sd in &hist.daily {
        if sd.offset >= -window && sd.offset <= 0 {
            if hist.promo_days.contains(&sd.offset) {
                continue;
            }
            units += sd.units;
        }
    }
    units
}

/// Weekday demand factors (Mon=0..Sun=6): how much each weekday deviates from
/// the flat average. Clamped to [0.3, 3.0]. Uses the trailing `window` days.
/// Denominators are CALENDAR weekday counts, not sale-day entries (sparse
/// sellers must not get inflated weekday factors).
pub fn weekday_factors(hist: &ProductHistory, window: i64) -> [f64; 7] {
    let mut units = [0.0f64; 7];
    let mut cal_days = [0i64; 7];
    for sd in &hist.daily {
        if sd.offset >= -window && sd.offset <= 0 {
            let dow = ((sd.offset % 7) + 7) % 7;
            units[dow as usize] += sd.units;
        }
    }
    // Calendar count of each weekday in the window (offset 0 = today's dow).
    for d in -window..=0 {
        let dow = ((d % 7) + 7) % 7;
        cal_days[dow as usize] += 1;
    }
    let mut factors = [1.0f64; 7];
    let total_units: f64 = units.iter().sum();
    if total_units <= 0.0 {
        return factors;
    }
    let total_days: i64 = cal_days.iter().sum();
    let avg = total_units / total_days as f64;
    for d in 0..7 {
        if cal_days[d] > 0 {
            let rate = units[d] / cal_days[d] as f64;
            factors[d] = (rate / avg).clamp(0.3, 3.0);
        }
    }
    factors
}

/// Trend guard: blend 30d and 90d rates, then clamp so a spike or dip can't
/// swing the order by more than ±50% of the 90d rate.
pub fn trend_adjusted(rate30: f64, rate90: f64) -> f64 {
    if rate90 <= 0.0 {
        return rate30;
    }
    let blended = 0.7 * rate30 + 0.3 * rate90;
    blended.clamp(0.5 * rate90, 1.5 * rate90)
}

/// Total units expected over the next `days` days: base rate × weekday factor
/// × promo uplift, summed per day. Promo windows apply ONLY on the days they
/// cover inside the window; weekday seasonality applies per calendar day.
pub fn forward_demand(
    base_rate: f64,
    factors: &[f64; 7],
    upcoming: &[(i64, i64)],
    promo_uplift: f64,
    days: i64,
    today_dow: i64,
) -> f64 {
    if days <= 0 {
        return 0.0;
    }
    let mut total = 0.0f64;
    for d in 0..days {
        let dow = ((today_dow + d) % 7 + 7) % 7;
        let mut factor = factors[dow as usize];
        let in_promo = upcoming.iter().any(|(s, e)| d >= *s && d <= *e);
        if in_promo {
            factor *= promo_uplift;
        }
        total += base_rate * factor;
    }
    total
}

/// Forward daily rate over the next `horizon` days: base rate × weekday
/// factor × promo uplift for each day, averaged (= forward_demand / days).
pub fn forward_rate(
    base_rate: f64,
    factors: &[f64; 7],
    upcoming: &[(i64, i64)],
    promo_uplift: f64,
    horizon: i64,
    today_dow: i64,
) -> f64 {
    if horizon <= 0 {
        return base_rate;
    }
    forward_demand(
        base_rate,
        factors,
        upcoming,
        promo_uplift,
        horizon,
        today_dow,
    ) / horizon as f64
}

/// Days until `available` stock (on-hand + on-order) runs out at the per-day
/// forward forecast (weekday factor × promo uplift). None = no forecast sales
/// (or more than 365 days out — effectively "never" for ordering purposes).
pub fn sellout_days(
    available: f64,
    base_rate: f64,
    factors: &[f64; 7],
    upcoming: &[(i64, i64)],
    promo_uplift: f64,
    today_dow: i64,
) -> Option<i64> {
    if available <= 0.0 {
        return Some(0);
    }
    if base_rate <= 0.0 {
        return None;
    }
    let mut cum = 0.0f64;
    for d in 0..365 {
        let dow = ((today_dow + d) % 7 + 7) % 7;
        let mut factor = factors[dow as usize];
        if upcoming.iter().any(|(s, e)| d >= *s && d <= *e) {
            factor *= promo_uplift;
        }
        cum += base_rate * factor;
        if cum >= available {
            return Some(d + 1);
        }
    }
    None
}

/// Learned promo uplift from history: promo-day rate ÷ non-promo rate,
/// clamped [1.0, 3.0]. Returns the default when not computable.
pub fn learned_promo_uplift(hist: &ProductHistory, default: f64) -> f64 {
    let mut promo_units = 0.0f64;
    let mut promo_days_cal = 0i64;
    let mut base_units = 0.0f64;
    for sd in &hist.daily {
        if sd.offset < -90 || sd.offset > 0 {
            continue;
        }
        if hist.promo_days.contains(&sd.offset) {
            promo_units += sd.units;
        } else {
            base_units += sd.units;
        }
    }
    for d in -90..=0 {
        if hist.promo_days.contains(&d) {
            promo_days_cal += 1;
        }
    }
    let base_days_cal = 91 - promo_days_cal;
    if promo_days_cal >= 3 && base_days_cal >= 10 && base_units > 0.0 {
        let pr = promo_units / promo_days_cal as f64;
        let br = base_units / base_days_cal as f64;
        if br > 0.0 {
            return (pr / br).clamp(1.0, 3.0);
        }
    }
    default
}

/// Suggested order quantity for one line, pack-rounded.
/// `needed_units` is the total forecast units required over the cover window
/// (lead + cycle), already including shrinkage — computed by the caller from
/// `forward_demand` so weekday/promo effects apply per-day, not via a
/// lead-window rate extrapolation.
/// `ignore_min_qty`/`ignore_max_qty` (global switches) independently skip the
/// Items.MinQty floor and Items.MaxQty cap while item-master min/max data is
/// being corrected.
pub fn suggested_qty(
    on_hand: f64,
    on_order: f64,
    needed_units: f64,
    pack_size: f64,
    min_qty: f64,
    max_qty: f64,
    no_order: bool,
    ignore_min_qty: bool,
    ignore_max_qty: bool,
) -> f64 {
    if no_order {
        return 0.0;
    }
    let pack = if pack_size > 0.0 { pack_size } else { 1.0 };
    let gap = needed_units - (on_hand + on_order);
    if gap <= 0.0 {
        return 0.0;
    }
    let mut qty = (gap / pack).ceil() * pack;
    if !ignore_min_qty {
        if min_qty > 0.0 && qty < min_qty {
            qty = min_qty;
        }
    }
    if !ignore_max_qty {
        if max_qty > 0.0 && qty > max_qty {
            qty = max_qty;
        }
    }
    (qty * 100.0).round() / 100.0
}

/// Compose a full line result.
pub fn compute_line(inp: &LineInput, hist: &ProductHistory, today_dow: i64) -> LineResult {
    let (r30, sale30) = baseline_rate(hist, 30);
    let (r90, _) = baseline_rate(hist, 90);
    let units7 = window_units(hist, 7);
    let units30 = window_units(hist, 30);
    let units90 = window_units(hist, 90);
    // No history at all → never auto-suggest; the operator decides (new,
    // replacement, or dead-but-active line). No dept-rate fallback here.
    let no_history = sale30 == 0 && r90 <= 0.0;
    let mut base = if no_history {
        0.0
    } else if sale30 < 5 {
        // Too few recent sale days — fall back to 90d, then dept rate.
        // Zero sales in the whole 30d window: halted/declining line — halve
        // the stale 90d rate rather than reorder at full speed.
        if r90 > 0.0 {
            if sale30 == 0 {
                r90 * 0.5
            } else {
                r90
            }
        } else {
            inp.dept_rate
        }
    } else {
        trend_adjusted(r30, r90)
    };
    if base <= 0.0 && !no_history {
        base = inp.dept_rate;
    }
    let factors = weekday_factors(hist, 90);
    let uplift = learned_promo_uplift(hist, inp.promo_uplift_default);
    let horizon = inp.lead_days.max(1);
    let rate_forward = forward_rate(
        base,
        &factors,
        &hist.upcoming_promos,
        uplift,
        horizon,
        today_dow,
    );
    // Cover-window demand: per-day weekday × promo uplift summed over the FULL
    // lead + cycle window (not the lead-window rate extrapolated).
    let cover_demand = forward_demand(
        base,
        &factors,
        &hist.upcoming_promos,
        uplift,
        inp.cover_days.max(0),
        today_dow,
    );
    let forecast_to_arrival = rate_forward * inp.lead_days as f64;
    let projected_stock = inp.on_hand + inp.on_order - forecast_to_arrival;
    let needed_units = cover_demand * (1.0 + inp.shrink_pct);
    let sellout_days = sellout_days(
        inp.on_hand + inp.on_order,
        base,
        &factors,
        &hist.upcoming_promos,
        uplift,
        today_dow,
    );
    let suggested = suggested_qty(
        inp.on_hand,
        inp.on_order,
        needed_units,
        inp.pack_size,
        inp.min_qty,
        inp.max_qty,
        inp.no_order,
        inp.ignore_min_qty,
        inp.ignore_max_qty,
    );
    LineResult {
        rate30: r30,
        rate90: r90,
        rate_forward,
        cover_demand,
        units7,
        units30,
        units90,
        sellout_days,
        sale_days30: sale30,
        forecast_to_arrival,
        projected_stock,
        suggested,
        promo_uplift_used: uplift,
        no_history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(daily: &[(i64, f64)], promo_days: &[i64], upcoming: &[(i64, i64)]) -> ProductHistory {
        ProductHistory {
            daily: daily
                .iter()
                .map(|(o, u)| SaleDay {
                    offset: *o,
                    units: *u,
                })
                .collect(),
            promo_days: promo_days.iter().copied().collect(),
            upcoming_promos: upcoming.to_vec(),
        }
    }

    #[test]
    fn baseline_excludes_promo_days() {
        // 30 days: 29 normal days at 10 units, 1 promo day at 100.
        let mut daily = Vec::new();
        for o in -30..=0 {
            daily.push((o, if o == -5 { 100.0 } else { 10.0 }));
        }
        let h = hist(&daily, &[-5], &[]);
        let (rate, sale_days) = baseline_rate(&h, 30);
        assert_eq!(rate, 10.0); // 300 / 30 non-promo days
        assert_eq!(sale_days, 30);
    }

    #[test]
    fn fallback_to_90d_when_sparse() {
        // Sales only 60-90 days ago; nothing in the last 30 days.
        let mut daily = Vec::new();
        for o in -90..=0 {
            daily.push((o, if o < -60 { 5.0 } else { 0.0 }));
        }
        let h = hist(&daily, &[], &[]);
        let (_r30, sale30) = baseline_rate(&h, 30);
        let (r90, _) = baseline_rate(&h, 90);
        assert_eq!(sale30, 0);
        let inp = LineInput {
            on_hand: 10.0,
            on_order: 0.0,
            pack_size: 6.0,
            min_qty: 0.0,
            max_qty: 0.0,
            no_order: false,
            lead_days: 5,
            cover_days: 12,
            shrink_pct: 0.01,
            promo_uplift_default: 1.5,
            dept_rate: 2.0,
            ignore_min_qty: false,
            ignore_max_qty: false,
        };
        let res = compute_line(&inp, &h, 0);
        // sale30 < 5 → base falls back to the 90d rate (not the dept rate)
        assert!(r90 > 0.0);
        assert!(res.rate_forward > 0.0);
        assert!(res.rate_forward <= 2.0);
    }

    #[test]
    fn halted_line_gets_half_rate() {
        // Strong sales 60-90 days ago, nothing in the last 30 → 30d rate 0.
        let mut daily = Vec::new();
        for o in -90..=0 {
            daily.push((o, if o < -60 { 10.0 } else { 0.0 }));
        }
        let h = hist(&daily, &[], &[]);
        let (_, sale30) = baseline_rate(&h, 30);
        assert_eq!(sale30, 0);
        let inp = LineInput {
            on_hand: 10.0,
            on_order: 0.0,
            pack_size: 6.0,
            min_qty: 0.0,
            max_qty: 0.0,
            no_order: false,
            lead_days: 5,
            cover_days: 12,
            shrink_pct: 0.01,
            promo_uplift_default: 1.5,
            dept_rate: 2.0,
            ignore_min_qty: false,
            ignore_max_qty: false,
        };
        let res = compute_line(&inp, &h, 0);
        // r90 ≈ 300/91 ≈ 3.30 → halved base ≈ 1.65 (not the full stale rate)
        assert!(res.rate_forward > 0.0);
        assert!(
            res.rate_forward < res.rate90,
            "halted line must be halved: {} vs {}",
            res.rate_forward,
            res.rate90
        );
    }

    #[test]
    fn sparse_seller_rate_is_calendar_based() {
        // 2 units sold on a single day in the window — rate must be 2/31,
        // NOT 2.0/day (the old entry-denominator bug inflated sparse sellers).
        let h = hist(&[(-2, 2.0)], &[], &[]);
        let (rate, sale_days) = baseline_rate(&h, 30);
        assert_eq!(sale_days, 1);
        assert!((rate - 2.0 / 31.0).abs() < 1e-9, "rate {}", rate);
    }

    #[test]
    fn window_units_sums_promo_excluded() {
        // 29 normal days at 10 + 1 promo day at 100 in the 30d window.
        let mut daily = Vec::new();
        for o in -30..=0 {
            daily.push((o, if o == -5 { 100.0 } else { 10.0 }));
        }
        let h = hist(&daily, &[-5], &[]);
        assert_eq!(window_units(&h, 30), 300.0); // 30 normal days, promo excluded
        assert_eq!(window_units(&h, 7), 70.0); // 8 offsets -7..=0, promo -5 excluded
        assert_eq!(window_units(&h, 90), 300.0); // 30 offsets + promo day excluded
                                                 // Window boundary: offset -8 must not count in a 7-day window.
        let h2 = hist(&[(-8, 5.0), (-7, 3.0), (0, 2.0)], &[], &[]);
        assert_eq!(window_units(&h2, 7), 5.0);
    }

    #[test]
    fn no_history_forces_zero_suggestion() {
        let h = hist(&[], &[], &[]); // no sales at all
        let inp = LineInput {
            on_hand: 0.0,
            on_order: 0.0,
            pack_size: 6.0,
            min_qty: 0.0,
            max_qty: 0.0,
            no_order: false,
            lead_days: 5,
            cover_days: 12,
            shrink_pct: 0.01,
            promo_uplift_default: 1.5,
            dept_rate: 20.0, // dept rate must NOT kick in
            ignore_min_qty: false,
            ignore_max_qty: false,
        };
        let res = compute_line(&inp, &h, 0);
        assert!(res.no_history);
        assert_eq!(res.rate_forward, 0.0);
        assert_eq!(res.suggested, 0.0);
    }

    #[test]
    fn trend_capped_at_50pct_of_90d() {
        let base = 10.0f64;
        let spike = trend_adjusted(30.0, base);
        assert!(spike <= 15.0, "spike {} must be capped at 15", spike);
        let dip = trend_adjusted(1.0, base);
        assert!(dip >= 5.0, "dip {} must be floored at 5", dip);
    }

    #[test]
    fn weekday_factors_peak() {
        // Sales only on one weekday (say Friday).
        let mut daily = Vec::new();
        for o in -90..=0 {
            // Friday = dow 4 (offset -2 is always 2 days before today's dow — use fixed mapping:
            // choose offsets so offset%7 maps to a single dow: pick o where (o mod 7) == 0
            daily.push((o, if o % 7 == 0 { 14.0 } else { 2.0 }));
        }
        let h = hist(&daily, &[], &[]);
        let f = weekday_factors(&h, 90);
        let max = f.iter().cloned().fold(0.0f64, f64::max);
        let min = f.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max > 1.5);
        assert!(min < 0.8);
    }

    #[test]
    fn rounding_to_pack() {
        // needed_units already includes shrink: 2*12*1.01 = 24.24.
        let q = suggested_qty(3.0, 0.0, 24.24, 6.0, 0.0, 0.0, false, false, false);
        assert_eq!(q, 24.0);
    }

    #[test]
    fn no_order_flag_and_min_max() {
        assert_eq!(
            suggested_qty(0.0, 0.0, 50.0, 1.0, 0.0, 0.0, true, false, false),
            0.0
        );
        let min_q = suggested_qty(0.0, 0.0, 10.0, 1.0, 12.0, 0.0, false, false, false); // needed 10 → raised to 12
        assert_eq!(min_q, 12.0);
        let max_q = suggested_qty(0.0, 0.0, 50.0, 1.0, 0.0, 30.0, false, false, false);
        assert_eq!(max_q, 30.0);
    }

    #[test]
    fn zero_gap_means_no_order() {
        assert_eq!(
            suggested_qty(100.0, 0.0, 24.24, 6.0, 0.0, 0.0, false, false, false),
            0.0
        );
    }

    #[test]
    fn ignore_flags_skip_min_floor() {
        // Lemon 4% shape: needed = 0.275 × 35 × 1.01 ≈ 9.72 → gap 6.12 → 7.
        // ignore_min_qty=true skips the floor (would otherwise raise to 25).
        let q = suggested_qty(3.6, 0.0, 9.72, 1.0, 25.0, 25.0, false, true, false);
        assert_eq!(q, 7.0);
    }

    #[test]
    fn ignore_flags_skip_max_cap() {
        // Green Apple shape: needed = 2.178 × 35 × 1.01 ≈ 77.0 → gap 56.6 → 57.
        // ignore_max_qty=true skips the cap (would otherwise cap to 25).
        let q = suggested_qty(20.4, 0.0, 77.0, 1.0, 25.0, 25.0, false, false, true);
        assert_eq!(q, 57.0);
    }

    #[test]
    fn min_max_applied_when_flags_off() {
        assert_eq!(
            suggested_qty(3.6, 0.0, 9.72, 1.0, 25.0, 25.0, false, false, false),
            25.0
        );
        assert_eq!(
            suggested_qty(20.4, 0.0, 77.0, 1.0, 25.0, 25.0, false, false, false),
            25.0
        );
    }

    #[test]
    fn no_order_wins_over_ignore_flags() {
        assert_eq!(
            suggested_qty(0.0, 0.0, 50.0, 1.0, 0.0, 0.0, true, true, true),
            0.0
        );
    }

    #[test]
    fn promo_lift_applied_in_forward_rate() {
        let _h = hist(&[], &[], &[(1, 5)]); // promo starts tomorrow, 5 days
        let factors = [1.0f64; 7];
        let f = forward_rate(10.0, &factors, &[(1, 5)], 2.0, 7, 0);
        // Days 1-5 at 2x (5 promo days), days 0,6 at 1x → (2*10 + 5*20)/7 = 120/7
        assert!((f - 120.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn forward_demand_sums_flat_days() {
        // Flat factors, no promo: 32 days × base.
        let f = forward_demand(2.0, &[1.0; 7], &[], 1.5, 32, 0);
        assert!((f - 64.0).abs() < 1e-9);
    }

    #[test]
    fn forward_demand_uplifts_only_promo_days() {
        // Promo days 1-5 of a 32-day window at 2x, rest at 1x → 270 + 100 = 370.
        let f = forward_demand(10.0, &[1.0; 7], &[(1, 5)], 2.0, 32, 0);
        assert!((f - 370.0).abs() < 1e-9);
    }

    #[test]
    fn forward_demand_ignores_promo_after_window() {
        // Promo starts day 10 of a 7-day window → no uplift at all.
        let f = forward_demand(10.0, &[1.0; 7], &[(10, 20)], 2.0, 7, 0);
        assert!((f - 70.0).abs() < 1e-9);
    }

    #[test]
    fn forward_demand_applies_weekday_factors() {
        // Mon=0.5, Tue=1.5, rest 1.0 → 2-day window = 2.0 × base.
        let mut factors = [1.0f64; 7];
        factors[0] = 0.5; // Mon
        factors[1] = 1.5; // Tue
        let f = forward_demand(10.0, &factors, &[], 1.5, 2, 0);
        assert!((f - 20.0).abs() < 1e-9);
    }

    #[test]
    fn forward_rate_is_forward_demand_over_days() {
        let f = forward_demand(10.0, &[1.0; 7], &[(1, 5)], 2.0, 7, 0);
        assert!((forward_rate(10.0, &[1.0; 7], &[(1, 5)], 2.0, 7, 0) - f / 7.0).abs() < 1e-9);
    }

    #[test]
    fn sellout_days_flat_rate() {
        assert_eq!(sellout_days(10.0, 1.0, &[1.0; 7], &[], 1.5, 0), Some(10));
        assert_eq!(sellout_days(5.0, 2.0, &[1.0; 7], &[], 1.5, 0), Some(3)); // 2+2+2 ≥ 5
        assert_eq!(sellout_days(0.0, 2.0, &[1.0; 7], &[], 1.5, 0), Some(0)); // already out
        assert_eq!(sellout_days(-1.0, 2.0, &[1.0; 7], &[], 1.5, 0), Some(0));
    }

    #[test]
    fn sellout_days_counts_promo_days_at_uplift() {
        // base 1/day, promo days 0-1 at 2x → 2,2,1,1,1,1,1,1 → ≥10 on day 8.
        assert_eq!(
            sellout_days(10.0, 1.0, &[1.0; 7], &[(0, 1)], 2.0, 0),
            Some(8)
        );
    }

    #[test]
    fn sellout_days_no_demand_or_huge_stock_is_none() {
        assert_eq!(sellout_days(10.0, 0.0, &[1.0; 7], &[], 1.5, 0), None); // no sales
        assert_eq!(sellout_days(1000.0, 1.0, &[1.0; 7], &[], 1.5, 0), None); // > 365 days
    }

    #[test]
    fn sellout_days_honours_weekday_factors() {
        // Mon=0.5, Tue=1.5; available 2 → Mon sells 0.5, Tue sells 1.5 → out Tue (2 days).
        let mut factors = [1.0f64; 7];
        factors[0] = 0.5;
        factors[1] = 1.5;
        assert_eq!(sellout_days(2.0, 1.0, &factors, &[], 1.5, 0), Some(2));
    }

    #[test]
    fn suggestion_uses_cover_demand_not_lead_window_rate() {
        // Weekend-heavy product: 0.3 units on Mon-Fri, 2.5 on Sat/Sun (history),
        // 2-day lead ordered on a Monday. Old math sized the whole cover at the
        // Mon+Tue rate (weekday factor ≈ 0.32) → would order ~nothing. New math
        // sums per-day factors over the full cover window → a real order.
        let mut daily = Vec::new();
        for o in -90..=0 {
            let dow = ((o % 7) + 7) % 7;
            daily.push((o, if dow >= 5 { 2.5 } else { 0.3 }));
        }
        let h = hist(&daily, &[], &[]);
        let inp = LineInput {
            on_hand: 10.0,
            on_order: 0.0,
            pack_size: 1.0,
            min_qty: 0.0,
            max_qty: 0.0,
            no_order: false,
            lead_days: 2,
            cover_days: 32,
            shrink_pct: 0.01,
            promo_uplift_default: 1.5,
            dept_rate: 1.0,
            ignore_min_qty: true,
            ignore_max_qty: true,
        };
        let res = compute_line(&inp, &h, 0); // Monday
                                             // rate_forward (lead 2 = Mon+Tue) ≈ base × weekday factor. cover_demand
                                             // sums the FULL 32-day window (24 weekdays + 8 weekend days), so the
                                             // ratio cover_demand / rate_forward ≈ 24 + 8×(2.69/0.32) ≈ 90, NOT the
                                             // old 32× cover extrapolation.
        let ratio = res.cover_demand / res.rate_forward;
        assert!(ratio > 80.0 && ratio < 100.0, "cover/lead ratio {}", ratio);
        // The weekend days the old math missed: cover demand exceeds the old
        // `rate_forward × cover` extrapolation by a wide margin.
        assert!(
            res.cover_demand - res.rate_forward * 32.0 > 10.0,
            "cover demand {} vs old extrapolation {}",
            res.cover_demand,
            res.rate_forward * 32.0
        );
        // And a real order results (old lead-window math would suggest ~1).
        assert!(
            res.suggested > 10.0,
            "cover-demand sizing must beat lead-window sizing: {}",
            res.suggested
        );
    }

    #[test]
    fn learned_uplift_from_history() {
        // 20 promo days at 30/day, 70 normal days at 10/day → uplift 3.0 (clamped)
        let mut daily = Vec::new();
        for o in -90..=0 {
            let promo = o >= -40 && o <= -21;
            daily.push((o, if promo { 30.0 } else { 10.0 }));
        }
        let promo_days: Vec<i64> = (-40..=-21).collect();
        let h = hist(&daily, &promo_days, &[]);
        assert_eq!(learned_promo_uplift(&h, 1.5), 3.0);
    }
}
