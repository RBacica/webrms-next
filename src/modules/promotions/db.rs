// Promotions module — READ-ONLY, engine-adaptive, over the local DB.
//
// Both pricing engines materialize into the same `promo_rules` table
// (kind 'special' | 'rbp_condition'), so list/effectiveness/items work
// identically in standalone/alongside/migrated modes.
//
// Effectiveness uses the same semantics as WebRMS R13: promo window =
// promo_units (LineType='S' attributed), base window = same-length window
// immediately before, uplift = promo/base. clip_range/base_window/
// promo_math/fifo_blend_avg_cost are ported VERBATIM from WebRMS.
use sqlx::sqlite::SqlitePool;

use super::rbp::{self, RbpCondition};

/// A unified active-promotion row (both engines).
#[derive(Debug, serde::Serialize, Clone)]
pub struct Promotion {
    pub id: String,
    pub description: String,
    /// "UPC" | "GROUP" | "SET" | "KIT"
    pub scope: String,
    pub product: String,
    pub price: f64,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub branch: Option<i32>,
    pub active: bool,
    pub engine: String,
}

/// Promo-effectiveness row (R13 logic, promo-unit attribution).
#[derive(Debug, serde::Serialize, Clone)]
pub struct PromoEffectiveness {
    pub id: String,
    pub description: String,
    pub upc: String,
    pub price: f64,
    pub promo_window: String,
    pub promo_units: f64,
    pub promo_net: f64,
    pub base_units: f64,
    pub base_net: f64,
    pub uplift_units: Option<f64>,
    pub uplift_net: Option<f64>,
}

/// One item line under a promotion (expandable special detail).
#[derive(Debug, serde::Serialize, Clone)]
pub struct PromotionItem {
    pub upc: String,
    pub description: String,
    /// Real SOH average cost: FIFO-layered from receipt history when the
    /// trail covers stock; blended with / falling back to Items.Cost.
    pub avg_cost: f64,
    /// Items.Price1 — normal retail price (pre-promo).
    pub price1: f64,
    /// (Price1 − promo_price) / Price1 × 100. None when Price1 = 0.
    pub discount_pct: Option<f64>,
    /// (promo_price − avg_cost) / promo_price × 100. None when no cost data.
    pub gp_pct: Option<f64>,
    /// How avg_cost was derived: "fifo" | "blend" | "master".
    pub cost_source: String,
}

/// Item detail for one promotion: the items its scope resolves to.
#[derive(Debug, serde::Serialize, Clone)]
pub struct PromotionItems {
    pub id: String,
    pub scope: String,
    pub product: String,
    /// false when the scope can't be resolved to Items rows.
    pub resolvable: bool,
    pub truncated: bool,
    pub items: Vec<PromotionItem>,
    /// For PROSET (bundle / 2-for) deals: the structured deal description.
    pub deal: Option<serde_json::Value>,
}

/// Which pricing engine the SOURCE runs — inferred from the materialized
/// promo_rules (any rbp_condition rows → Rules_Based), with settings override.
/// Defaults to Standard when unknown.
pub async fn pricing_engine(pool: &SqlitePool) -> String {
    let v: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'pricing_engine'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match v.as_deref() {
        Some(x) if x.trim().eq_ignore_ascii_case("rules_based") => "Rules_Based".into(),
        Some(x) if x.trim().eq_ignore_ascii_case("standard") => "Standard".into(),
        _ => {
            // fall back to inference from the data
            let rbp: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM promo_rules WHERE kind = 'rbp_condition'",
            )
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            if rbp > 0 { "Rules_Based".into() } else { "Standard".into() }
        }
    }
}

/// Unified active-promotion list for the given branch (empty = all).
pub async fn list_promotions(pool: &SqlitePool, branch: Option<i32>) -> anyhow::Result<Vec<Promotion>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT id, COALESCE(description, ''), COALESCE(condition_type, ''), \
                COALESCE(sequence_match, ''), CAST(COALESCE(adjustment_value, 0) AS REAL), \
                effective_start, effective_end, branch_scope, is_active, kind \
         FROM promo_rules WHERE 1=1",
    );
    if let Some(b) = branch {
        qb.push(" AND (branch_scope IS NULL OR branch_scope = ").push_bind(b).push(")");
    }
    qb.push(" ORDER BY is_active DESC, description");
    let rows: Vec<(i64, String, String, String, f64, Option<String>, Option<String>, Option<i32>, i64, String)> =
        qb.build_query_as().fetch_all(pool).await?;

    let scope_of = |ctype: &str, sm: &str| -> String {
        if ctype.trim().eq_ignore_ascii_case("PROSET") {
            "SET".into()
        } else if sm.contains('|') {
            "GROUP".into()
        } else {
            "UPC".into()
        }
    };

    Ok(rows
        .into_iter()
        .map(
            |(id, desc, ctype, sm, price, from, to, br, active, kind)| Promotion {
                id: format!("pc-{id}"),
                description: desc,
                scope: scope_of(&ctype, &sm),
                product: sm,
                price,
                from_date: from,
                to_date: to,
                branch: br,
                active: active == 1,
                engine: if kind == "rbp_condition" { "Rules_Based".into() } else { "Standard".into() },
            },
        )
        .collect())
}

/// Units + net for a UPC in [start, end): promo window = promo_units (S-lines
/// attributed at promo price), net = revenue of those units.
async fn line_sales(
    pool: &SqlitePool,
    upc: &str,
    start: &str,
    end: &str,
    branch: Option<i64>,
) -> anyhow::Result<(f64, f64)> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT CAST(COALESCE(SUM(units), 0) AS REAL), CAST(COALESCE(SUM(revenue), 0) AS REAL) \
         FROM sales_daily WHERE upc = ",
    );
    qb.push_bind(upc)
        .push(" AND sale_date >= ").push_bind(start)
        .push(" AND sale_date < ").push_bind(end);
    if let Some(b) = branch {
        qb.push(" AND branch_id = ").push_bind(b);
    }
    let row: (f64, f64) = qb.build_query_as().fetch_one(pool).await?;
    Ok(row)
}

/// Effective-window computation (R13 semantics, ported verbatim).
fn clip_range(from_d: &str, to_d: &str, from: &str, to: &str) -> (String, String) {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
    let p_start = parse(from_d).unwrap_or_else(|| parse(from).unwrap());
    let p_end_raw = parse(to_d).unwrap_or_else(|| parse(to).unwrap());
    // end is inclusive in Specials; make it exclusive for the query
    let p_end = p_end_raw + chrono::Duration::days(1);
    let req_start = parse(from).unwrap();
    let req_end = parse(to).unwrap() + chrono::Duration::days(1);
    let start = p_start.max(req_start);
    let end = p_end.min(req_end);
    (
        start.format("%Y-%m-%d").to_string(),
        end.format("%Y-%m-%d").to_string(),
    )
}

/// Same-length window immediately before the promo window.
fn base_window(p_start: &str, p_end: &str) -> (String, String) {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let start = parse(p_start).unwrap();
    let end = parse(p_end).unwrap();
    let len = (end - start).num_days();
    let b_end = start;
    let b_start = start - chrono::Duration::days(len);
    (
        b_start.format("%Y-%m-%d").to_string(),
        b_end.format("%Y-%m-%d").to_string(),
    )
}

/// Promo effectiveness over local sales_daily, for the requested range.
/// Branches: RBP-style (condition branch_scope) per-condition; sales measured
/// branch-scoped when `branch` given, else across all branches.
pub async fn promotion_effectiveness(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i32>,
) -> anyhow::Result<Vec<PromoEffectiveness>> {
    let conds: Vec<RbpCondition> = rbp::active_conditions(pool, branch).await?;
    let branch_i64 = branch.map(|b| b as i64);

    let mut results = Vec::new();
    for c in &conds {
        let from_d = c.from.trim();
        let to_d = c.to.trim();
        if from_d.is_empty() {
            continue; // open-ended has no measurable base window
        }
        let (p_start, p_end) = clip_range(from_d, to_d, from, to);
        let (b_start, b_end) = base_window(&p_start, &p_end);
        if p_start >= p_end {
            continue;
        }
        for upc in &c.upcs {
            let (pu, pn) = line_sales(pool, upc, &p_start, &p_end, branch_i64).await?;
            let (bu, bn) = line_sales(pool, upc, &b_start, &b_end, branch_i64).await?;
            let uplift = if bu > 0.0 { Some(pu / bu) } else { None };
            let uplift_net = if bn > 0.0 { Some(pn / bn) } else { None };
            results.push(PromoEffectiveness {
                id: format!("pc-{}", c.condition_id),
                description: c.description.clone(),
                upc: upc.clone(),
                price: c.price,
                promo_window: format!("{} → {}", p_start, p_end),
                promo_units: pu,
                promo_net: pn,
                base_units: bu,
                base_net: bn,
                uplift_units: uplift,
                uplift_net,
            });
        }
    }
    results.sort_by(|a, b| b.promo_units.partial_cmp(&a.promo_units).unwrap_or(std::cmp::Ordering::Equal));
    Ok(results)
}

/// Discount % + GP % for a promo line (GST-exclusive base) — ported verbatim.
fn promo_math(price1: f64, avg_cost: f64, promo_price: f64) -> (Option<f64>, Option<f64>) {
    let discount = if price1 > 0.0 {
        Some((promo_price - price1) / price1 * 100.0)
    } else {
        None
    };
    let gp = if promo_price > 0.0 && avg_cost > 0.0 {
        let excl = promo_price / 1.15; // GST 15%
        Some((excl - avg_cost) / excl * 100.0)
    } else {
        None
    };
    (discount, gp)
}

/// FIFO-blended average cost from receipt layers, blended with master cost.
/// Ported verbatim from WebRMS (fifo_blend_avg_cost).
fn fifo_blend_avg_cost(layers: &[(f64, f64)], soh: f64, master_cost: f64) -> (f64, &'static str) {
    if soh <= 0.0 || layers.is_empty() {
        return (master_cost, "master");
    }
    let mut remaining = soh;
    let mut cost = 0.0f64;
    let mut covered = 0.0f64;
    for (qty, unit_cost) in layers {
        if *qty <= 0.0 {
            continue;
        }
        let take = remaining.min(*qty);
        if take > 0.0 {
            cost += take * unit_cost;
            covered += take;
            remaining -= take;
        }
        if remaining <= 0.0 {
            break;
        }
    }
    if remaining <= 0.0 {
        (cost / covered, "fifo")
    } else {
        // blend: covered portion at fifo cost, rest at master
        let total = cost + remaining * master_cost;
        (total / soh, "blend")
    }
}

/// Item detail rows for one promotion (resolved UPCs + cost/price math).
pub async fn promotion_items(
    pool: &SqlitePool,
    id: &str,
    branch: Option<i32>,
) -> anyhow::Result<PromotionItems> {
    let cond_id: i64 = id.strip_prefix("pc-").and_then(|s| s.parse().ok()).unwrap_or(0);
    let row: Option<(String, String, String, f64, Option<String>, Option<String>, Option<i32>, Option<String>)> =
        sqlx::query_as(
            "SELECT COALESCE(description, ''), COALESCE(condition_type, ''), \
                    COALESCE(sequence_match, ''), CAST(COALESCE(adjustment_value, 0) AS REAL), \
                    effective_start, effective_end, branch_scope, adjustment_type \
             FROM promo_rules WHERE id = ?1",
        )
        .bind(cond_id)
        .fetch_optional(pool)
        .await?;
    let Some((desc, ctype, sm, price, _from, _to, br, adj_type)) = row else {
        return Ok(PromotionItems {
            id: id.into(),
            scope: String::new(),
            product: String::new(),
            resolvable: false,
            truncated: false,
            items: vec![],
            deal: None,
        });
    };
    let scope = if ctype.trim().eq_ignore_ascii_case("PROSET") { "SET" } else { "UPC" };
    let upcs = rbp::resolve_condition_upcs(pool, &sm).await?;
    let eff_branch = branch.or(br);

    // PROSET deal structure (bundle / 2-for)
    let deal = if ctype.trim().eq_ignore_ascii_case("PROSET") {
        let (kind, left, _right) = rbp::classify(&sm);
        if kind == "proset" {
            if let Ok(set_id) = left.parse::<i64>() {
                match rbp::resolve_proset_deal(
                    pool, set_id,
                    adj_type.as_deref().unwrap_or(""),
                    price, &desc, "", "", eff_branch, &sm,
                ).await {
                    Ok(d) => Some(serde_json::to_value(d).unwrap_or(serde_json::Value::Null)),
                    Err(_) => None,
                }
            } else { None }
        } else { None }
    } else { None };

    // item details
    let mut items = Vec::new();
    for upc in &upcs {
        let meta: Option<(String, f64, f64, f64)> = sqlx::query_as(
            "SELECT COALESCE(description, ''), CAST(COALESCE(cost, 0) AS REAL), \
                    CAST(COALESCE(price1, 0) AS REAL), CAST(COALESCE(pack_units, 1) AS REAL) \
             FROM items WHERE upc = ?1 AND is_active = 1",
        )
        .bind(upc)
        .fetch_optional(pool)
        .await?;
        let Some((idesc, master_cost, price1, _pack)) = meta else { continue };
        // FIFO layers from receipt_lines (goods-in 'G' + 'I'), latest first
        let layers: Vec<(f64, f64)> = sqlx::query_as(
            "SELECT rl.quantity, COALESCE(rl.unit_cost, 0) FROM receipt_lines rl \
             JOIN receipts r ON r.id = rl.receipt_id \
             WHERE rl.upc = ?1 AND r.trans_type IN ('G','I') AND rl.quantity > 0 \
             ORDER BY r.id DESC LIMIT 200",
        )
        .bind(upc)
        .fetch_all(pool)
        .await?;
        let soh: f64 = match branch {
            Some(b) => sqlx::query_scalar("SELECT qty FROM stock_current WHERE upc = ?1 AND branch_id = ?2")
                .bind(upc).bind(b).fetch_optional(pool).await?.unwrap_or(0.0),
            None => 0.0, // no single branch → master cost (no reliable SOH)
        };
        let (avg_cost, source) = if soh > 0.0 && !layers.is_empty() {
            fifo_blend_avg_cost(&layers, soh, master_cost)
        } else {
            (master_cost, "master")
        };
        let (discount_pct, gp_pct) = promo_math(price1, avg_cost, price);
        items.push(PromotionItem {
            upc: upc.clone(),
            description: idesc,
            avg_cost,
            price1,
            discount_pct,
            gp_pct,
            cost_source: source.into(),
        });
    }
    let truncated = items.len() > 100;
    if truncated {
        items.truncate(100);
    }
    Ok(PromotionItems {
        id: id.into(),
        scope: scope.into(),
        product: sm,
        resolvable: !upcs.is_empty(),
        truncated,
        items,
        deal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_range_respects_request_bounds() {
        // special runs Apr 1 - Apr 30; request Jul 1-31 -> no overlap
        let (s, e) = clip_range("2026-04-01", "2026-04-30", "2026-07-01", "2026-07-31");
        assert!(s > e, "empty window expected, got {} > {}", s, e);
        // special runs Jun 15 - Jul 15; request Jul 1-31 -> Jul 1 - Jul 16
        let (s, e) = clip_range("2026-06-15", "2026-07-15", "2026-07-01", "2026-07-31");
        assert_eq!(s, "2026-07-01");
        assert_eq!(e, "2026-07-16");
    }

    #[test]
    fn base_window_same_length_before() {
        let (b, e) = base_window("2026-07-01", "2026-07-16");
        assert_eq!(b, "2026-06-16");
        assert_eq!(e, "2026-07-01");
    }

    #[test]
    fn clip_and_base_window_compose() {
        // promo Jul 20-Aug 16 inclusive; request wider range
        let (s, e) = clip_range("2026-07-20", "2026-08-16", "2026-07-01", "2026-08-31");
        assert_eq!(s, "2026-07-20");
        assert_eq!(e, "2026-08-17");
        // base = same length before (28 days: Jul 20 - 28 = Jun 22)
        let (b, be) = base_window(&s, &e);
        assert_eq!(b, "2026-06-22");
        assert_eq!(be, "2026-07-20");
    }

    #[test]
    fn promo_math_normal_discount_and_gp() {
        // Price1 $5.00, cost $3.00, promo $4.00 -> 20% OFF (deduction, negative),
        // GP on GST-exclusive promo price: (4/1.15 - 3)/(4/1.15) = 13.75%
        let (d, g) = promo_math(5.0, 3.0, 4.0);
        let d = d.unwrap();
        let g = g.unwrap();
        assert!((d + 20.0).abs() < 1e-9, "discount {d}");
        assert!((g - 13.75).abs() < 1e-9, "gp {g}");
    }

    #[test]
    fn promo_math_bombaysap_gst_case() {
        // Price1 $76.99, cost $46.11, promo $64.99
        // discount = (64.99-76.99)/76.99 = -15.6% (deduction)
        // GP = (64.99/1.15 - 46.11)/(64.99/1.15) = 18.4%
        let (d, g) = promo_math(76.99, 46.11, 64.99);
        assert!((d.unwrap() + 15.6).abs() < 0.1);
        assert!((g.unwrap() - 18.4).abs() < 0.1);
    }

    #[test]
    fn promo_math_zero_price1_no_discount() {
        let (d, g) = promo_math(0.0, 3.0, 4.0);
        assert!(d.is_none());
        assert!((g.unwrap() - 13.75).abs() < 1e-9);
    }

    #[test]
    fn promo_math_zero_cost_no_gp() {
        let (d, g) = promo_math(5.0, 0.0, 4.0);
        assert!(g.is_none());
        assert!((d.unwrap() + 20.0).abs() < 1e-9);
    }

    #[test]
    fn promo_math_above_price_positive_discount() {
        let (d, g) = promo_math(4.0, 3.0, 5.0);
        assert!((d.unwrap() - 25.0).abs() < 1e-9);
        assert!((g.unwrap() - 31.0).abs() < 1e-9);
    }

    #[test]
    fn promo_math_zero_promo_price_no_gp() {
        let (_, g) = promo_math(5.0, 3.0, 0.0);
        assert!(g.is_none());
    }

    #[test]
    fn fifo_fully_covered_single_layer() {
        let layers = [(156.0, 40.73)];
        let (avg, src) = fifo_blend_avg_cost(&layers, 94.0, 40.35);
        assert_eq!(src, "fifo");
        assert!((avg - 40.73).abs() < 1e-9);
    }

    #[test]
    fn fifo_fully_covered_multi_layer() {
        let layers = [(60.0, 10.0), (80.0, 20.0)];
        let (avg, src) = fifo_blend_avg_cost(&layers, 100.0, 15.0);
        assert_eq!(src, "fifo");
        assert!((avg - 14.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_partial_blends_with_master() {
        let layers = [(40.0, 10.0)];
        let (avg, src) = fifo_blend_avg_cost(&layers, 100.0, 20.0);
        assert_eq!(src, "blend");
        assert!((avg - 16.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_partial_crosses_layer_boundary() {
        let layers = [(30.0, 10.0), (30.0, 20.0)];
        let (avg, src) = fifo_blend_avg_cost(&layers, 50.0, 99.0);
        assert_eq!(src, "fifo");
        assert!((avg - 14.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_no_receipts_uses_master() {
        let (avg, src) = fifo_blend_avg_cost(&[], 10.0, 42.0);
        assert_eq!(src, "master");
        assert!((avg - 42.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_zero_soh_uses_master() {
        let (avg, src) = fifo_blend_avg_cost(&[(100.0, 5.0)], 0.0, 42.0);
        assert_eq!(src, "master");
        assert!((avg - 42.0).abs() < 1e-9);
    }

    #[test]
    fn fifo_zero_quantity_layers_ignored() {
        let layers = [(0.0, 50.0), (10.0, 5.0)];
        let (avg, src) = fifo_blend_avg_cost(&layers, 10.0, 42.0);
        assert_eq!(src, "fifo");
        assert!((avg - 5.0).abs() < 1e-9);
    }
}
