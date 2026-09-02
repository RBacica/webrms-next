// Payables DB layer — local SQLite port of WebRMS payables.
// Bills = ap_invoices (connector-materialized APInv), net of paid_ledger marks
// and supplier-returns ('Z' receipts are credits, not bills).
// Due dates come from supplier_terms; unconfigured → EOM+20 fallback.

use chrono::Datelike;
use sqlx::sqlite::SqlitePool;

#[derive(Debug, serde::Serialize, Clone)]
pub struct Supplier {
    pub code: String,
    pub label: String,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct Branch {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct Invoice {
    pub branch: i64,
    pub supplier_code: String,
    pub invoice_number: String,
    pub description: String,
    pub invoice_date: String,
    pub invoice_amount: f64,
    pub paid_amount: f64,
    pub po_number: String,
    pub tax_amount: f64,
    pub logged: String,
    pub due_date: String,
    /// true when the supplier's terms are unconfigured (fallback EOM+20 used)
    pub terms_unset: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct InvoiceQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub branch: Option<String>,
    pub supplier: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct PayablesExportRow {
    pub branch: String,
    pub supplier_code: String,
    pub invoice_number: String,
    pub description: String,
    pub invoice_date: String,
    pub invoice_amount: f64,
    pub po_number: String,
    pub tax_amount: f64,
    pub due_date: String,
}

/// EOM+20 fallback when terms are unconfigured — ported from supplier_config.rs.
pub fn fallback_due_date(invoice_date: &str) -> String {
    let parsed = chrono::NaiveDate::parse_from_str(invoice_date, "%Y-%m-%d");
    let d = match parsed {
        Ok(d) => d,
        Err(_) => return invoice_date.to_string(),
    };
    // end of month
    let eom = if d.month() == 12 {
        chrono::NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1)
    };
    let Some(eom) = eom else { return invoice_date.to_string() };
    let due = eom - chrono::Duration::days(1) + chrono::Duration::days(20);
    due.format("%Y-%m-%d").to_string()
}

pub async fn due_date_for(
    pool: &SqlitePool,
    supplier_code: &str,
    invoice_date: &str,
) -> anyhow::Result<(String, bool)> {
    let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT term_type, term_days FROM supplier_terms WHERE supplier_code = ?1 AND configured = 1",
    )
    .bind(supplier_code)
    .fetch_optional(pool)
    .await?;
    let Some((term_type, term_days)) = row else {
        return Ok((fallback_due_date(invoice_date), true));
    };
    let days = term_days.unwrap_or(0) as i64;
    let parsed = match chrono::NaiveDate::parse_from_str(invoice_date, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Ok((invoice_date.to_string(), false)),
    };
    let due = match term_type.as_deref() {
        Some("EOM") => {
            let eom = if parsed.month() == 12 {
                chrono::NaiveDate::from_ymd_opt(parsed.year() + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(parsed.year(), parsed.month() + 1, 1)
            };
            match eom {
                Some(eom) => eom - chrono::Duration::days(1) + chrono::Duration::days(days),
                None => parsed,
            }
        }
        _ => parsed + chrono::Duration::days(days),
    };
    Ok((due.format("%Y-%m-%d").to_string(), false))
}

pub async fn get_suppliers(pool: &SqlitePool) -> anyhow::Result<Vec<Supplier>> {
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT code, last_name, first_name FROM suppliers WHERE is_active = 1 ORDER BY last_name ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(code, last_name, first_name)| {
            let label = match (last_name.as_deref().unwrap_or("").is_empty(), first_name.as_deref().unwrap_or("").is_empty()) {
                (false, false) => format!("{} - {} ({})", code, last_name.unwrap_or_default(), first_name.unwrap_or_default()),
                (false, true) => format!("{} - {}", code, last_name.unwrap_or_default()),
                (true, false) => format!("{} - ({})", code, first_name.unwrap_or_default()),
                (true, true) => code.clone(),
            };
            Supplier { code, label }
        })
        .collect())
}

pub async fn get_branches(pool: &SqlitePool) -> anyhow::Result<Vec<Branch>> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM branches WHERE is_ho = 0 ORDER BY name ASC")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(id, name)| Branch { id, name }).collect())
}

/// Bills due = ap_invoices (unpaid, in range, branch/supplier filtered),
/// net of paid_ledger marks.
pub async fn get_bills(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
    supplier: Option<&str>,
) -> anyhow::Result<Vec<Invoice>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT ai.branch_id, COALESCE(s.code, ''), ai.invoice_number, ai.description, \
                ai.invoice_date, ai.invoice_amount, ai.paid_amount, ai.po_number, \
                ai.tax_amount1, ai.logged \
         FROM ap_invoices ai LEFT JOIN suppliers s ON s.id = ai.supplier_id \
         WHERE ai.invoice_date >= ",
    );
    qb.push_bind(from).push(" AND ai.invoice_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND ai.branch_id = ").push_bind(b);
    }
    if let Some(sup) = supplier.filter(|s: &&str| !s.is_empty() && *s != "ALL") {
        qb.push(" AND s.code = ").push_bind(sup);
    }
    // exclude fully-paid
    qb.push(" AND ai.paid_amount < ai.invoice_amount");
    qb.push(" ORDER BY ai.logged DESC");
    let rows: Vec<(i64, String, String, String, String, f64, f64, String, f64, String)> =
        qb.build_query_as().fetch_all(pool).await?;

    // paid marks keyed by (branch, supplier_code, invoice_number)
    let paid: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT COALESCE(branch_id, 0), supplier_code, invoice_no FROM paid_ledger",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for (branch_id, supplier_code, inv_no, desc, inv_date, amount, paid_amt, po, tax, logged) in rows {
        let marked = paid.iter().any(|(b, s, i)| {
            *b == branch_id && s == &supplier_code && i == &inv_no
        });
        if marked {
            continue;
        }
        let (due_date, terms_unset) = due_date_for(pool, &supplier_code, &inv_date).await?;
        out.push(Invoice {
            branch: branch_id,
            supplier_code,
            invoice_number: inv_no,
            description: desc,
            invoice_date: inv_date,
            invoice_amount: amount,
            paid_amount: paid_amt,
            po_number: po,
            tax_amount: tax,
            logged,
            due_date,
            terms_unset,
        });
    }
    Ok(out)
}

/// Supplier returns / credits = receipts 'Z' (credit note vs supplier).
pub async fn get_returns(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
    supplier: Option<&str>,
) -> anyhow::Result<Vec<Invoice>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT r.branch_id, COALESCE(s.code, ''), r.invoice_no, 'Supplier return', \
                r.logged, r.total_cost, CAST(0 AS REAL), '', CAST(0 AS REAL), r.logged \
         FROM receipts r LEFT JOIN suppliers s ON s.id = r.supplier_id \
         WHERE r.trans_type = 'Z' AND r.logged >= ",
    );
    qb.push_bind(from).push(" AND r.logged < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND r.branch_id = ").push_bind(b);
    }
    if let Some(sup) = supplier.filter(|s: &&str| !s.is_empty() && *s != "ALL") {
        qb.push(" AND s.code = ").push_bind(sup);
    }
    qb.push(" ORDER BY r.logged DESC");
    let rows: Vec<(i64, String, String, String, String, f64, f64, String, f64, String)> =
        qb.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(branch, supplier_code, inv_no, desc, inv_date, amount, paid, po, tax, logged)| Invoice {
                branch,
                supplier_code,
                invoice_number: inv_no,
                description: desc,
                invoice_date: inv_date,
                invoice_amount: amount,
                paid_amount: paid,
                po_number: po,
                tax_amount: tax,
                logged,
                due_date: String::new(),
                terms_unset: false,
            },
        )
        .collect())
}

/// Mark rows paid → paid_ledger (app-owned; replicated via outbox DOWN).
pub async fn mark_paid(pool: &SqlitePool, rows: &[PaidRow]) -> anyhow::Result<usize> {
    let mut added = 0usize;
    let mut tx = pool.begin().await?;
    for r in rows {
        if r.invoice_number.is_empty() {
            continue;
        }
        let res = sqlx::query(
            "INSERT OR IGNORE INTO paid_ledger (id, branch_id, supplier_code, invoice_no, paid_at, amount, note, origin_install) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(r.branch_id)
        .bind(&r.supplier_code)
        .bind(&r.invoice_number)
        .bind(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string())
        .bind(r.amount)
        .bind(r.note.as_deref().unwrap_or(""))
        .bind("local")
        .execute(&mut *tx)
        .await?;
        added += res.rows_affected() as usize;
    }
    tx.commit().await?;
    Ok(added)
}

pub async fn get_paid(pool: &SqlitePool) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows: Vec<(i64, String, String, String, f64)> = sqlx::query_as(
        "SELECT COALESCE(branch_id, 0), supplier_code, invoice_no, paid_at, amount FROM paid_ledger ORDER BY paid_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(b, s, i, at, amt)| {
            serde_json::json!({
                "branch": b, "supplier_code": s, "invoice_number": i,
                "paid_at": at, "amount": amt,
            })
        })
        .collect())
}

#[derive(Debug, serde::Deserialize, Clone)]
pub struct PaidRow {
    pub branch_id: Option<i64>,
    pub supplier_code: String,
    pub invoice_number: String,
    pub amount: f64,
    pub note: Option<String>,
}

// ── supplier terms config (supplier_terms; HoS-write, BoS read-only) ───────

#[derive(Debug, serde::Serialize, Clone)]
pub struct SupplierConfigEntry {
    pub code: String,
    pub label: String,
    pub term_type: Option<String>,
    pub term_days: Option<i64>,
    pub order_type: Option<String>,
    pub payment_type: Option<String>,
    pub configured: bool,
}

pub async fn get_config(pool: &SqlitePool) -> anyhow::Result<Vec<SupplierConfigEntry>> {
    let rows: Vec<(String, String, Option<String>, Option<i64>, Option<String>, Option<String>, i64)> =
        sqlx::query_as(
            "SELECT s.code, s.name, st.term_type, st.term_days, st.order_type, st.payment_type, COALESCE(st.configured, 0) \
             FROM suppliers s LEFT JOIN supplier_terms st ON st.supplier_code = s.code \
             WHERE s.is_active = 1 ORDER BY s.last_name ASC",
        )
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(code, name, tt, td, ot, pt, conf)| SupplierConfigEntry {
                code,
                label: name,
                term_type: tt,
                term_days: td,
                order_type: ot,
                payment_type: pt,
                configured: conf == 1,
            },
        )
        .collect())
}

pub async fn save_one(
    pool: &SqlitePool,
    supplier_code: &str,
    term_type: &str,
    term_days: Option<i64>,
    order_type: &str,
    payment_type: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO supplier_terms (supplier_code, term_type, term_days, order_type, payment_type, configured, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1, 'app', ?6) \
         ON CONFLICT(supplier_code) DO UPDATE SET \
           term_type=excluded.term_type, term_days=excluded.term_days, \
           order_type=excluded.order_type, payment_type=excluded.payment_type, \
           configured=1, source='app', updated_at=excluded.updated_at",
    )
    .bind(supplier_code)
    .bind(term_type)
    .bind(term_days)
    .bind(order_type)
    .bind(payment_type)
    .bind(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string())
    .execute(pool)
    .await?;
    Ok(())
}
