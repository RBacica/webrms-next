// Shared Rules_Based pricing (RBP) promo resolver — LOCAL SQLite port.
//
// The New HoS runs the RBP engine — promotions live in PricingCondition /
// PricingSequence / PricingGroup / PricingProductSet, NOT the Standard
// `Specials` table. The connector materializes conditions into promo_rules
// and the set/group chain into pricing_sets / pricing_groups (migration
// 0004), so resolution works fully locally (standalone mode).
//
// Active conditions carry a `SequenceMatch` in one of three forms:
//   RETAIL (ConditionType 'RETAIL' / Sequence 'U'):   bare UPC   e.g. "5010196065047"
//   LOCAL  (ConditionType 'LOCAL' / Sequence 'B|U'):  "<groupid>|<upc>" e.g. "17|9419227009122"
//   PROSET (ConditionType 'PROSET' / Sequence 'SET|*'): "<setid>|<line>" e.g. "64|1"
//
// Resolution to actual item UPCs:
//   RETAIL -> the UPC itself.
//   LOCAL  -> the UPC after the pipe.
//   PROSET -> SetID -> pricing_sets.line -> GroupID ->
//             pricing_groups.data_key (a UPC, Type='Items').
//
// All functions here are read-only. `classify` is ported VERBATIM from
// WebRMS (same hard-won edge cases).
use sqlx::sqlite::SqlitePool;

/// A resolved active condition: its item UPCs + effective window + the UI scope.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RbpCondition {
    pub condition_id: i64,
    pub description: String,
    pub sequence_match: String,
    pub scope: String, // "UPC" | "SET"
    pub upcs: Vec<String>,
    pub from: String, // "YYYY-MM-DD" (may be empty)
    pub to: String,   // "YYYY-MM-DD" (may be empty)
    pub price: f64,
    pub branch: Option<i32>,
}

/// Parse a SequenceMatch into its raw parts. Returns (kind, left, right).
/// kind: "bare" | "local" | "proset" | "other"
pub fn classify(sm: &str) -> (&'static str, String, String) {
    if sm.is_empty() {
        return ("other", String::new(), String::new());
    }
    if let Some(rest) = sm.strip_prefix("U|") {
        // tokenised UPC "U|<upc>"
        return ("local", String::new(), rest.trim().to_string());
    }
    if let Some(idx) = sm.find('|') {
        let left = sm[..idx].trim().to_string();
        let right = sm[idx + 1..].trim().to_string();
        // PROSET form is "<setid>|<line>" (set id is numeric); LOCAL is
        // "<groupid>|<upc>". Distinguish by the right side being a UPC length.
        if left.parse::<i64>().is_ok() {
            // Both have a numeric left. A set reference is `setid|line` where
            // line is a small int (1..N). A LOCAL group is `groupid|upc` where
            // the right side is a full-length barcode. Treat a short numeric
            // right (< 6 chars) as a set-line reference.
            if right.len() <= 5 && right.parse::<i64>().is_ok() {
                return ("proset", left, right);
            }
            return ("local", left, right);
        }
        return ("other", left, right);
    }
    ("bare", String::new(), sm.trim().to_string())
}

/// Resolve the item UPCs a sequence-match covers, locally.
pub async fn resolve_condition_upcs(
    pool: &SqlitePool,
    sequence_match: &str,
) -> anyhow::Result<Vec<String>> {
    let (kind, left, right) = classify(sequence_match);
    match kind {
        "bare" => Ok(vec![sequence_match.trim().to_string()]),
        "local" => Ok(vec![right]),
        "proset" => {
            let set_id: i64 = left.parse().unwrap_or(0);
            if set_id == 0 {
                return Ok(Vec::new());
            }
            resolve_set_items(pool, set_id).await
        }
        _ => Ok(Vec::new()),
    }
}

/// Expand a PricingProductSet (SetID) to its item UPCs via its groups — local.
async fn resolve_set_items(pool: &SqlitePool, set_id: i64) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT g.data_key \
         FROM pricing_sets ps \
         JOIN pricing_groups g ON g.group_id = ps.group_id \
         WHERE ps.set_id = ?1 AND g.is_active = 1 AND g.type = 'Items' \
           AND g.data_key <> ''",
    )
    .bind(set_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Item UPCs in a PricingGroup — local.
pub async fn resolve_group_items(pool: &SqlitePool, group_id: i64) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT data_key FROM pricing_groups \
         WHERE group_id = ?1 AND is_active = 1 AND type = 'Items' AND data_key <> ''",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Display name for a PricingGroup — local.
pub async fn group_name(pool: &SqlitePool, group_id: i64) -> anyhow::Result<String> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT description FROM pricing_groups WHERE group_id = ?1 AND is_active = 1 LIMIT 1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    Ok(name.unwrap_or_default())
}

/// One line of a PROSET deal: how many of a group qualify, and what the
/// group contains.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DealLine {
    pub qty: i64,           // MinQuantity (e.g. 1 spirit, 2 cokes)
    pub group_id: i64,      // PricingGroup.GroupID
    pub group_name: String, // PricingGroup.Description
    pub items: Vec<String>, // DataKey UPCs in the group
}

/// A PROSET deal: a 2-for-$ price or a bundle, with its qualifying lines.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RbpDeal {
    pub set_id: i64,
    pub sequence: String, // "SET|SPT" (2-for) | "SET|STL" (bundle)
    pub deal_type: String, // "two_for" | "bundle"
    pub description: String,
    /// The deal price for 2-for-$ (`ABS`) deals; None for percent bundles.
    pub deal_price: Option<f64>,
    /// A percent discount for `%` bundle lines; None otherwise.
    pub discount_pct: Option<f64>,
    pub lines: Vec<DealLine>,
    /// Union of all qualifying item UPCs across lines.
    pub upcs: Vec<String>,
    pub from: String,
    pub to: String,
    pub branch: Option<i32>,
}

/// Resolve a PROSET set into its deal structure (lines + qty + groups) — local.
/// `adjustment_type`/`adjustment_value` come from the FIRST active condition
/// referencing this set (its line 1), which carries the deal price/discount.
pub async fn resolve_proset_deal(
    pool: &SqlitePool,
    set_id: i64,
    adjustment_type: &str,
    adjustment_value: f64,
    description: &str,
    from: &str,
    to: &str,
    branch: Option<i32>,
    sequence: &str,
) -> anyhow::Result<RbpDeal> {
    let rows: Vec<(i64, f64, f64, i64)> = sqlx::query_as(
        "SELECT set_line, min_qty, max_qty, group_id FROM pricing_sets \
         WHERE set_id = ?1 ORDER BY set_line",
    )
    .bind(set_id)
    .fetch_all(pool)
    .await?;

    let mut lines = Vec::new();
    let mut upcs = Vec::new();
    for (_line, minq, _maxq, group_id) in rows {
        if group_id == 0 {
            continue;
        }
        let items = resolve_group_items(pool, group_id).await?;
        let gname = group_name(pool, group_id).await?;
        for u in &items {
            if !upcs.iter().any(|x| x == u) {
                upcs.push(u.clone());
            }
        }
        lines.push(DealLine {
            qty: minq as i64,
            group_id,
            group_name: gname,
            items,
        });
    }

    let at = adjustment_type.trim().to_uppercase();
    // The live "SET|SPT"/"SET|STL" sequence string isn't materialized locally;
    // the adjustment type carries the deal shape: ABS price = 2-for-$,
    // % = percent bundle.
    let (deal_type, deal_price, discount_pct) = if at == "ABS" {
        ("two_for".to_string(), Some(adjustment_value), None)
    } else if at == "%" {
        ("bundle".to_string(), None, Some(adjustment_value))
    } else {
        ("bundle".to_string(), None, None)
    };

    Ok(RbpDeal {
        set_id,
        sequence: sequence.to_string(),
        deal_type,
        description: description.to_string(),
        deal_price,
        discount_pct,
        lines,
        upcs,
        from: from.to_string(),
        to: to.to_string(),
        branch,
    })
}

/// All active conditions (from local promo_rules) with resolved UPCs + windows.
/// `branch`: Some(n) scopes to that branch (branch_scope NULL or = n);
/// None = all branches. Returns conditions that resolve to at least one UPC.
pub async fn active_conditions(
    pool: &SqlitePool,
    branch: Option<i32>,
) -> anyhow::Result<Vec<RbpCondition>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT id, COALESCE(description, ''), COALESCE(sequence_match, ''), \
                CAST(COALESCE(adjustment_value, 0) AS REAL), \
                COALESCE(effective_start, ''), COALESCE(effective_end, ''), \
                branch_scope, condition_type \
         FROM promo_rules WHERE is_active = 1",
    );
    if let Some(b) = branch {
        qb.push(" AND (branch_scope IS NULL OR branch_scope = ").push_bind(b).push(")");
    }
    qb.push(" ORDER BY description");
    let rows: Vec<(i64, String, String, f64, String, String, Option<i32>, Option<String>)> =
        qb.build_query_as().fetch_all(pool).await?;

    let mut out = Vec::new();
    for (cid, desc, sm, price, from, to, br, ctype) in rows {
        if cid == 0 {
            continue;
        }
        let scope = if ctype.as_deref().unwrap_or("").trim().eq_ignore_ascii_case("PROSET") {
            "SET"
        } else {
            "UPC"
        };
        let upcs = resolve_condition_upcs(pool, &sm).await?;
        if upcs.is_empty() {
            continue;
        }
        out.push(RbpCondition {
            condition_id: cid,
            description: desc,
            sequence_match: sm,
            scope: scope.to_string(),
            upcs,
            from: from.trim().to_string(),
            to: to.trim().to_string(),
            price,
            branch: br,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bare_upc() {
        let (k, l, r) = classify("5010196065047");
        assert_eq!(k, "bare");
        assert_eq!(l, "");
        assert_eq!(r, "5010196065047");
    }

    #[test]
    fn classify_local_group_upc() {
        // LOCAL form: <groupid>|<upc>
        let (k, l, r) = classify("17|9419227009122");
        assert_eq!(k, "local");
        assert_eq!(l, "17");
        assert_eq!(r, "9419227009122");
    }

    #[test]
    fn classify_proset_set_line() {
        // PROSET form: <setid>|<line> where line is a small int
        let (k, l, r) = classify("64|1");
        assert_eq!(k, "proset");
        assert_eq!(l, "64");
        assert_eq!(r, "1");
    }

    #[test]
    fn classify_upc_pipe_form() {
        let (k, _l, r) = classify("U|5010196065047");
        assert_eq!(k, "local");
        assert_eq!(r, "5010196065047");
    }

    #[test]
    fn classify_empty() {
        let (k, _, _) = classify("");
        assert_eq!(k, "other");
    }
}
