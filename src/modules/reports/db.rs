// Reports DB layer — local SQLite port of WebRMS reports.
// All sales windows read from sales_daily (connector-aggregated); stock from
// stock_current; GP optionally uplifted by scanback/rebate receipts
// (rebate_ledger × rebate_contracts) per the design (C2).
use chrono::Datelike;
use sqlx::sqlite::SqlitePool;

#[derive(Debug, serde::Serialize, Clone)]
pub struct DailyRow {
    pub date: String,
    pub txns: i64,
    pub gross_excl_gst: f64,
    pub gross_total: f64,
    pub net: f64,
    pub cost: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct DailyReport {
    pub from: String,
    pub to: String,
    pub branch: Option<i64>,
    pub daily: Vec<DailyRow>,
    pub totals: DailyRow,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct ProductSales {
    pub upc: String,
    pub name: String,
    pub subdept: String,
    pub units: f64,
    pub gross_excl_gst: f64,
    pub gross_total: f64,
    pub cost: f64,
    pub net: f64,
    /// gross excl GST − cost (profitability)
    pub margin_amt: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct DeptSales {
    pub dept_id: i64,
    pub dept_name: String,
    pub target_margin: f64,
    pub units: f64,
    pub gross_excl_gst: f64,
    pub gross_total: f64,
    pub cost: f64,
    pub net: f64,
    pub products: Vec<ProductSales>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct DeptWeekly {
    pub dept_id: String,
    pub dept_name: String,
    pub this_week_gross: f64,
    pub last_week_gross: f64,
    pub delta_pct: Option<f64>,
    pub avg_12wk: Option<f64>,
    pub last_week_full_gross: f64,
    pub avg_pct: Option<f64>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct TopMover {
    pub upc: String,
    pub name: String,
    pub dept: String,
    pub units: f64,
    pub net: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct DeptMovers {
    pub dept_id: i64,
    pub dept_name: String,
    pub movers: Vec<TopMover>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct OverviewSales {
    pub today: f64,
    pub yesterday: f64,
    pub last_week_same_day: f64,
    pub this_week: f64,
    pub last_week: f64,
    pub four_wk_avg: f64,
    pub today_vs_yesterday_pct: Option<f64>,
    pub today_vs_last_week_pct: Option<f64>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct OverviewStock {
    pub items: i64,        // distinct UPCs with a current stock row
    pub value: f64,        // Σ qty × cost
    pub sellout_low: i64,  // items with sellout < 7 days (fast)
    pub stockout: i64,     // items with qty <= 0
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct OverviewResponse {
    pub as_of: String,
    pub branch: Option<i64>,
    pub sales: OverviewSales,
    pub stock: OverviewStock,
    /// scanback/rebate received in the window (app-only tracking, C2)
    pub scanback_received: f64,
    /// gross profit incl. scanback uplift, for the 7-day window
    pub gp_incl_scanback: f64,
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn iso(d: chrono::NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

async fn sum_window(pool: &SqlitePool, from: &str, to: &str, branch: Option<i64>) -> anyhow::Result<(f64, f64, f64)> {
    // (gross_total, cost, units) over sales_daily in [from, to)
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT CAST(COALESCE(SUM(revenue), 0) AS REAL), CAST(COALESCE(SUM(cost_amount), 0) AS REAL), \
                CAST(COALESCE(SUM(units), 0) AS REAL) \
         FROM sales_daily WHERE sale_date >= ",
    );
    qb.push_bind(from).push(" AND sale_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND branch_id = ").push_bind(b);
    }
    let row: (f64, f64, f64) = qb.build_query_as().fetch_one(pool).await?;
    Ok(row)
}

async fn scanback_window(pool: &SqlitePool, from: &str, to: &str, branch: Option<i64>) -> anyhow::Result<f64> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT CAST(COALESCE(SUM(rl.received_amount), 0) AS REAL) \
         FROM rebate_ledger rl \
         JOIN rebate_contracts rc ON rc.id = rl.contract_id \
         WHERE rl.received_date >= ",
    );
    qb.push_bind(from).push(" AND rl.received_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND COALESCE(rl.branch_id, 0) = ").push_bind(b);
    }
    let v: f64 = qb.build_query_scalar().fetch_one(pool).await?;
    Ok(v)
}

// ── queries ──────────────────────────────────────────────────────────────────

/// Daily summary: per-day rows + totals over [from, to).
pub async fn daily_summary(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
) -> anyhow::Result<DailyReport> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT sale_date, COUNT(DISTINCT branch_id || '|' || upc), \
                CAST(SUM(revenue) AS REAL), CAST(SUM(cost_amount) AS REAL), CAST(SUM(units) AS REAL) \
         FROM sales_daily WHERE sale_date >= ",
    );
    qb.push_bind(from).push(" AND sale_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND branch_id = ").push_bind(b);
    }
    qb.push(" GROUP BY sale_date ORDER BY sale_date");
    let rows: Vec<(String, i64, f64, f64, f64)> = qb.build_query_as().fetch_all(pool).await?;
    let mut daily = Vec::new();
    for (date, txns, gross_total, cost, _units) in rows {
        daily.push(DailyRow {
            date,
            txns,
            gross_excl_gst: gross_total / 1.15,
            gross_total,
            net: gross_total,
            cost,
        });
    }
    let totals = DailyRow {
        date: "TOTAL".into(),
        txns: daily.iter().map(|r| r.txns).sum(),
        gross_excl_gst: daily.iter().map(|r| r.gross_excl_gst).sum(),
        gross_total: daily.iter().map(|r| r.gross_total).sum(),
        net: daily.iter().map(|r| r.net).sum(),
        cost: daily.iter().map(|r| r.cost).sum(),
    };
    Ok(DailyReport {
        from: from.into(),
        to: to.into(),
        branch,
        daily,
        totals,
    })
}

/// Dept sales: sales_daily × items × departments, grouped per dept/product.
pub async fn dept_sales(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
    dept: Option<i64>,
) -> anyhow::Result<Vec<DeptSales>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT COALESCE(i.department_id, 0), COALESCE(d.name, 'Unknown'), COALESCE(d.target_margin, 0), \
                sd_upc.upc, COALESCE(i.description, sd_upc.upc), COALESCE(d.name, ''), \
                SUM(sd_upc.units), CAST(SUM(sd_upc.revenue) AS REAL), CAST(SUM(sd_upc.cost_amount) AS REAL), \
                CAST(SUM(sd_upc.line_margin) AS REAL) \
         FROM sales_daily sd_upc \
         LEFT JOIN items i ON i.upc = sd_upc.upc \
         LEFT JOIN departments d ON d.id = i.department_id \
         WHERE sd_upc.sale_date >= ",
    );
    qb.push_bind(from).push(" AND sd_upc.sale_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND sd_upc.branch_id = ").push_bind(b);
    }
    if let Some(d) = dept {
        qb.push(" AND i.department_id = ").push_bind(d);
    }
    qb.push(" AND (d.name IS NULL OR d.name NOT IN ('Non Sales','EPay')) \
             GROUP BY i.department_id, d.name, d.target_margin, sd_upc.upc, i.description \
             ORDER BY d.name, SUM(sd_upc.revenue) DESC");
    let rows: Vec<(i64, String, f64, String, String, String, f64, f64, f64, f64)> =
        qb.build_query_as().fetch_all(pool).await?;

    let mut depts: Vec<DeptSales> = Vec::new();
    for (dept_id, dept_name, target_margin, upc, name, subdept, units, gross_total, cost, margin) in rows {
        // find or create the dept bucket
        let bucket = match depts.iter_mut().find(|d| d.dept_id == dept_id) {
            Some(b) => b,
            None => {
                depts.push(DeptSales {
                    dept_id,
                    dept_name: dept_name.clone(),
                    target_margin,
                    units: 0.0,
                    gross_excl_gst: 0.0,
                    gross_total: 0.0,
                    cost: 0.0,
                    net: 0.0,
                    products: Vec::new(),
                });
                depts.last_mut().unwrap()
            }
        };
        let gross_excl = gross_total / 1.15;
        bucket.units += units;
        bucket.gross_excl_gst += gross_excl;
        bucket.gross_total += gross_total;
        bucket.cost += cost;
        bucket.net += gross_total;
        bucket.products.push(ProductSales {
            upc,
            name,
            subdept,
            units,
            gross_excl_gst: gross_excl,
            gross_total,
            cost,
            net: gross_total,
            margin_amt: margin,
        });
    }
    Ok(depts)
}

/// Overview: matched sales windows (today / yesterday / last-week same day /
/// this week / last week / 4-wk avg), stock snapshot, scanback + GP uplift.
pub async fn overview(pool: &SqlitePool, branch: Option<i64>) -> anyhow::Result<OverviewResponse> {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let last_week_same = today - chrono::Duration::days(7);
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let last_monday = monday - chrono::Duration::days(7);
    let f = iso;

    let (t_today, _, _) = sum_window(pool, &f(today), &f(today + chrono::Duration::days(1)), branch).await?;
    let (t_yday, _, _) = sum_window(pool, &f(yesterday), &f(today), branch).await?;
    let (t_lwsd, _, _) = sum_window(pool, &f(last_week_same), &f(last_week_same + chrono::Duration::days(1)), branch).await?;
    let (t_week, _, _) = sum_window(pool, &f(monday), &f(today + chrono::Duration::days(1)), branch).await?;
    let (t_last_week, _, _) = sum_window(pool, &f(last_monday), &f(monday), branch).await?;
    // 4-week avg of the same weekday
    let mut four_total = 0.0f64;
    for w in 1..=4 {
        let d = today - chrono::Duration::days(7 * w);
        let (v, _, _) = sum_window(pool, &f(d), &f(d + chrono::Duration::days(1)), branch).await?;
        four_total += v;
    }
    let four_wk_avg = four_total / 4.0;

    let pct = |a: f64, b: f64| if b > 0.0 { Some((a - b) / b * 100.0) } else { None };

    // stock snapshot (latest per UPC across branches, or branch-scoped)
    let (stock_items, stock_value, stockout) = {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*), CAST(COALESCE(SUM(x.qty * COALESCE(i.cost, 0)), 0) AS REAL), \
                    SUM(CASE WHEN x.qty <= 0 THEN 1 ELSE 0 END) \
             FROM (SELECT upc, qty FROM stock_current",
        );
        if let Some(b) = branch {
            qb.push(" WHERE branch_id = ").push_bind(b);
        }
        qb.push(" GROUP BY upc, qty) x \
                 LEFT JOIN items i ON i.upc = x.upc");
        let row: (i64, f64, i64) = qb.build_query_as().fetch_one(pool).await?;
        row
    };

    // 7-day window for scanback + GP
    let wk_from = f(today - chrono::Duration::days(6));
    let wk_to = f(today + chrono::Duration::days(1));
    let (wk_gross, wk_cost, _) = sum_window(pool, &wk_from, &wk_to, branch).await?;
    let scanback = scanback_window(pool, &wk_from, &wk_to, branch).await?;
    let gp_incl_scanback = if wk_gross > 0.0 {
        (wk_gross - wk_cost + scanback) / wk_gross * 100.0
    } else {
        0.0
    };

    Ok(OverviewResponse {
        as_of: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        branch,
        sales: OverviewSales {
            today: t_today,
            yesterday: t_yday,
            last_week_same_day: t_lwsd,
            this_week: t_week,
            last_week: t_last_week,
            four_wk_avg,
            today_vs_yesterday_pct: pct(t_today, t_yday),
            today_vs_last_week_pct: pct(t_today, t_lwsd),
        },
        stock: OverviewStock {
            items: stock_items,
            value: stock_value,
            sellout_low: 0, // sellout needs forecast rates; filled by UI
            stockout,
        },
        scanback_received: scanback,
        gp_incl_scanback,
    })
}

/// Top movers by net over the window, optionally within one dept.
pub async fn top_movers(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
    dept: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<TopMover>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT sd.upc, COALESCE(i.description, sd.upc), COALESCE(d.name, ''), \
                CAST(SUM(sd.units) AS REAL), CAST(SUM(sd.revenue) AS REAL) \
         FROM sales_daily sd \
         LEFT JOIN items i ON i.upc = sd.upc \
         LEFT JOIN departments d ON d.id = i.department_id \
         WHERE sd.sale_date >= ",
    );
    qb.push_bind(from).push(" AND sd.sale_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND sd.branch_id = ").push_bind(b);
    }
    if let Some(d) = dept {
        qb.push(" AND i.department_id = ").push_bind(d);
    }
    qb.push(" GROUP BY sd.upc, i.description, d.name \
             ORDER BY SUM(sd.revenue) DESC LIMIT ").push_bind(limit);
    let rows: Vec<(String, String, String, f64, f64)> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(upc, name, dept, units, net)| TopMover { upc, name, dept, units, net }).collect())
}

/// Dept movers: top 5 per department over the window.
pub async fn dept_movers(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
) -> anyhow::Result<Vec<DeptMovers>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT i.department_id, COALESCE(d.name, 'Unknown'), sd.upc, COALESCE(i.description, sd.upc), \
                CAST(SUM(sd.units) AS REAL), CAST(SUM(sd.revenue) AS REAL) \
         FROM sales_daily sd \
         LEFT JOIN items i ON i.upc = sd.upc \
         LEFT JOIN departments d ON d.id = i.department_id \
         WHERE sd.sale_date >= ",
    );
    qb.push_bind(from).push(" AND sd.sale_date < ").push_bind(to);
    if let Some(b) = branch {
        qb.push(" AND sd.branch_id = ").push_bind(b);
    }
    qb.push(" GROUP BY i.department_id, d.name, sd.upc, i.description \
             ORDER BY i.department_id, SUM(sd.revenue) DESC");
    let rows: Vec<(Option<i64>, String, String, String, f64, f64)> =
        qb.build_query_as().fetch_all(pool).await?;

    let mut out: Vec<DeptMovers> = Vec::new();
    for (dept_id, dept_name, upc, name, units, net) in rows {
        let dept_id = dept_id.unwrap_or(0);
        let bucket = match out.iter_mut().find(|d| d.dept_id == dept_id) {
            Some(b) => b,
            None => {
                out.push(DeptMovers { dept_id, dept_name: dept_name.clone(), movers: Vec::new() });
                out.last_mut().unwrap()
            }
        };
        if bucket.movers.len() < 5 {
            bucket.movers.push(TopMover { upc, name, dept: dept_name.clone(), units, net });
        }
    }
    Ok(out)
}

/// Dept weekly: this week vs last week vs 12-wk average per department,
/// with a Total row appended (dept_id "0", name "Total").
pub async fn dept_weekly(pool: &SqlitePool, branch: Option<i64>) -> anyhow::Result<Vec<DeptWeekly>> {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let this_from = monday;
    let this_to = today + chrono::Duration::days(1);
    let last_from = monday - chrono::Duration::days(7);
    let last_to = this_from;
    let avg12_from = monday - chrono::Duration::days(12 * 7);

    let mut qb = sqlx::QueryBuilder::new(
        "SELECT COALESCE(i.department_id, 0), COALESCE(d.name, 'Unknown'), \
                CAST(COALESCE(SUM(CASE WHEN sd.sale_date >= ",
    );
    // this week
    qb.push_bind(iso(this_from)).push(" AND sd.sale_date < ").push_bind(iso(this_to))
        .push(" THEN sd.revenue ELSE 0 END), 0) AS REAL), \
                CAST(COALESCE(SUM(CASE WHEN sd.sale_date >= ")
        .push_bind(iso(last_from)).push(" AND sd.sale_date < ").push_bind(iso(last_to))
        .push(" THEN sd.revenue ELSE 0 END), 0) AS REAL), \
                CAST(COALESCE(SUM(CASE WHEN sd.sale_date >= ")
        .push_bind(iso(avg12_from)).push(" AND sd.sale_date < ").push_bind(iso(this_from))
        .push(" THEN sd.revenue ELSE 0 END), 0) AS REAL) \
         FROM sales_daily sd \
         LEFT JOIN items i ON i.upc = sd.upc \
         LEFT JOIN departments d ON d.id = i.department_id \
         WHERE sd.sale_date >= ");
    qb.push_bind(iso(avg12_from)).push(" AND sd.sale_date < ").push_bind(iso(this_to));
    if let Some(b) = branch {
        qb.push(" AND sd.branch_id = ").push_bind(b);
    }
    qb.push(" AND (d.name IS NULL OR d.name NOT IN ('Non Sales','EPay')) \
             GROUP BY i.department_id, d.name ORDER BY 3 DESC");
    let rows: Vec<(i64, String, f64, f64, f64)> = qb.build_query_as().fetch_all(pool).await?;

    let mut out = Vec::new();
    let (mut t_this, mut t_last, mut t_avg12) = (0.0f64, 0.0f64, 0.0f64);
    for (dept_id, dept_name, this_week, last_week, avg12_raw) in rows {
        let avg12 = avg12_raw / 12.0;
        let delta = if last_week > 0.0 { Some((this_week - last_week) / last_week * 100.0) } else { None };
        let avg_pct = if avg12 > 0.0 { Some((last_week - avg12) / avg12 * 100.0) } else { None };
        t_this += this_week;
        t_last += last_week;
        t_avg12 += avg12;
        out.push(DeptWeekly {
            dept_id: dept_id.to_string(),
            dept_name,
            this_week_gross: this_week,
            last_week_gross: last_week,
            delta_pct: delta,
            avg_12wk: Some(avg12),
            last_week_full_gross: last_week,
            avg_pct,
        });
    }
    out.push(DeptWeekly {
        dept_id: "0".into(),
        dept_name: "Total".into(),
        this_week_gross: t_this,
        last_week_gross: t_last,
        delta_pct: if t_last > 0.0 { Some((t_this - t_last) / t_last * 100.0) } else { None },
        avg_12wk: Some(t_avg12),
        last_week_full_gross: t_last,
        avg_pct: if t_avg12 > 0.0 { Some((t_last - t_avg12) / t_avg12 * 100.0) } else { None },
    });
    Ok(out)
}

// ── R-parity ports (old WebRMS Reports view, over the local DB) ──────────────

#[derive(Debug, serde::Serialize, Clone)]
pub struct StockProduct {
    pub upc: String,
    pub name: String,
    pub subdept: String,
    pub on_hand: f64,
    pub price: f64,
    pub retail_value: f64, // on_hand × price ÷ 1.15 (excl GST, matches old report)
    pub cost_value: f64,
    pub units_30: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct StockDept {
    pub dept_name: String,
    pub items: i64,
    pub units: f64,
    pub retail_value: f64,
    pub cost_value: f64,
    pub gp_value: f64,
    pub products: Vec<StockProduct>,
}

/// R-stock: stock valuation by dept from stock_current × items.cost/price.
/// branch = None aggregates all branches' on-hand per UPC.
pub async fn stock_valuation(pool: &SqlitePool, branch: Option<i64>) -> anyhow::Result<Vec<StockDept>> {
    // 30d units per upc (matches the old report's 30-day window)
    let units30: std::collections::HashMap<String, f64> = sqlx::query_as::<_, (String, f64)>(
        "SELECT upc, COALESCE(SUM(units),0) FROM sales_daily \
         WHERE sale_date >= date('now','-30 days') GROUP BY upc",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();

    let items: Vec<(String, String, String, f64, f64, f64)> = if let Some(b) = branch {
        sqlx::query_as(
            "SELECT i.upc, COALESCE(i.description,''), COALESCE(d.name,''), \
                    CAST(COALESCE(NULLIF(i.cost,0), i.cost_ave, i.purchase_cost, 0) AS REAL), \
                    CAST(COALESCE(i.price1,0) AS REAL), \
                    CAST(COALESCE(sc.qty,0) AS REAL) \
             FROM items i \
             LEFT JOIN departments d ON d.id = i.department_id \
             LEFT JOIN stock_current sc ON sc.upc = i.upc AND sc.branch_id = ?1 \
             WHERE i.is_active = 1 AND i.non_stock = 0 \
               AND COALESCE(d.name,'') NOT IN ('Non Sales','EPay') \
             ORDER BY d.name, i.description",
        )
        .bind(b)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT i.upc, COALESCE(i.description,''), COALESCE(d.name,''), \
                    CAST(COALESCE(NULLIF(i.cost,0), i.cost_ave, i.purchase_cost, 0) AS REAL), \
                    CAST(COALESCE(i.price1,0) AS REAL), \
                    CAST(COALESCE((SELECT SUM(sc.qty) FROM stock_current sc WHERE sc.upc = i.upc),0) AS REAL) \
             FROM items i \
             LEFT JOIN departments d ON d.id = i.department_id \
             WHERE i.is_active = 1 AND i.non_stock = 0 \
               AND COALESCE(d.name,'') NOT IN ('Non Sales','EPay') \
             ORDER BY d.name, i.description",
        )
        .fetch_all(pool)
        .await?
    };

    let mut depts: Vec<StockDept> = Vec::new();
    let mut cur: Option<String> = None;
    let mut products: Vec<StockProduct> = Vec::new();
    for (upc, name, dept, cost, price, qty) in items {
        if cur.as_deref() != Some(dept.as_str()) {
            if let Some(c) = &cur {
                finish_stock_dept(&mut depts, c, &mut products);
            }
            cur = Some(dept);
        }
        products.push(StockProduct {
            upc: upc.clone(),
            name,
            subdept: String::new(),
            on_hand: qty,
            price,
            retail_value: qty * price / 1.15,
            cost_value: qty * cost,
            units_30: units30.get(&upc).copied().unwrap_or(0.0),
        });
    }
    if let Some(c) = &cur {
        finish_stock_dept(&mut depts, c, &mut products);
    }
    Ok(depts)
}

fn finish_stock_dept(depts: &mut Vec<StockDept>, name: &str, products: &mut Vec<StockProduct>) {
    let retail: f64 = products.iter().map(|p| p.retail_value).sum();
    let cost: f64 = products.iter().map(|p| p.cost_value).sum();
    depts.push(StockDept {
        dept_name: name.to_string(),
        items: products.len() as i64,
        units: products.iter().map(|p| p.on_hand).sum(),
        retail_value: retail,
        cost_value: cost,
        gp_value: retail - cost,
        products: std::mem::take(products),
    });
}

/// R-receipts: GRN ↔ AP reconciliation per supplier over a period.
/// goods-in ('G') and returns ('Z') come from receipts; AP from ap_invoices.
#[derive(Debug, serde::Serialize, Clone)]
pub struct ReceiptRow {
    pub supplier_code: String,
    pub supplier_name: String,
    pub goods_in: f64,     // total cost of 'G' receipts
    pub returns: f64,      // 'Z' credit notes
    pub net_grn: f64,      // goods_in − returns
    pub ap_invoiced: f64,  // ap_invoices.invoice_amount in the window
    pub variance: f64,     // ap_invoiced − net_grn
}

pub async fn receipts_report(
    pool: &SqlitePool,
    from: &str,
    to: &str,
    branch: Option<i64>,
) -> anyhow::Result<Vec<ReceiptRow>> {
    if let Some(_b) = branch {
        // single-branch variant
        let rows: Vec<(String, String, f64, f64, f64)> = sqlx::query_as(
            "SELECT COALESCE(s.code,''), COALESCE(s.name,''), \
                    CAST(COALESCE((SELECT SUM(total_cost) FROM receipts r WHERE r.supplier_id = s.id \
                       AND r.trans_type = 'G' AND r.logged >= ?1 AND r.logged < date(?2,'+1 day') \
                       AND r.branch_id = ?3),0) AS REAL), \
                    CAST(COALESCE((SELECT SUM(total_cost) FROM receipts r WHERE r.supplier_id = s.id \
                       AND r.trans_type = 'Z' AND r.logged >= ?1 AND r.logged < date(?2,'+1 day') \
                       AND r.branch_id = ?3),0) AS REAL), \
                    CAST(COALESCE((SELECT SUM(invoice_amount) FROM ap_invoices a WHERE a.supplier_id = s.id \
                       AND a.invoice_date >= ?1 AND a.invoice_date < date(?2,'+1 day') \
                       AND a.branch_id = ?3),0) AS REAL) \
             FROM suppliers s WHERE s.is_active = 1 ORDER BY s.code",
        )
        .bind(from).bind(to).bind(_b)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(c, n, g, z, a)| ReceiptRow {
            supplier_code: c.clone(), supplier_name: n.clone(),
            goods_in: g, returns: z, net_grn: g - z, ap_invoiced: a,
            variance: a - (g - z),
        }).collect())
    } else {
        let rows: Vec<(String, String, f64, f64, f64)> = sqlx::query_as(
            "SELECT COALESCE(s.code,''), COALESCE(s.name,''), \
                    CAST(COALESCE((SELECT SUM(total_cost) FROM receipts r WHERE r.supplier_id = s.id \
                       AND r.trans_type = 'G' AND r.logged >= ?1 AND r.logged < date(?2,'+1 day')),0) AS REAL), \
                    CAST(COALESCE((SELECT SUM(total_cost) FROM receipts r WHERE r.supplier_id = s.id \
                       AND r.trans_type = 'Z' AND r.logged >= ?1 AND r.logged < date(?2,'+1 day')),0) AS REAL), \
                    CAST(COALESCE((SELECT SUM(invoice_amount) FROM ap_invoices a WHERE a.supplier_id = s.id \
                       AND a.invoice_date >= ?1 AND a.invoice_date < date(?2,'+1 day')),0) AS REAL) \
             FROM suppliers s WHERE s.is_active = 1 ORDER BY s.code",
        )
        .bind(from).bind(to)
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(c, n, g, z, a)| ReceiptRow {
            supplier_code: c.clone(), supplier_name: n.clone(),
            goods_in: g, returns: z, net_grn: g - z, ap_invoiced: a,
            variance: a - (g - z),
        }).collect())
    }
}

/// R-stocktakes: stocktake export history from stocktake_runs with the costed
/// variance totals (G-3) + per-run counted lines.
#[derive(Debug, serde::Serialize, Clone)]
pub struct StocktakeRunLine {
    pub upc: String,
    pub description: Option<String>,
    pub stock_on_hand: f64,
    pub counted: f64,
    pub unit_cost: Option<f64>,
    pub variance_units: f64,
    pub variance_cost: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct StocktakeRow {
    pub id: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub branch_id: Option<i64>,
    pub count_file: Option<String>,
    pub ticket_file: Option<String>,
    pub shrink_total: f64,
    pub overage_total: f64,
    pub lines: Vec<StocktakeRunLine>,
}

pub async fn stocktakes_report(pool: &SqlitePool) -> anyhow::Result<Vec<StocktakeRow>> {
    let rows: Vec<(String, String, Option<String>, String, Option<i64>, Option<String>, Option<String>, f64, f64)> =
        sqlx::query_as(
            "SELECT id, started_at, completed_at, status, branch_id, count_file, ticket_file, \
                    CAST(COALESCE(shrink_total,0) AS REAL), CAST(COALESCE(overage_total,0) AS REAL) \
             FROM stocktake_runs ORDER BY started_at DESC LIMIT 100",
        )
        .fetch_all(pool)
        .await?;
    if rows.is_empty() {
        return Ok(vec![]);
    }
    // one batched query for all runs' lines
    let line_rows: Vec<(String, String, Option<String>, f64, f64, Option<f64>, f64, f64)> =
        sqlx::query_as(
            "SELECT run_id, upc, description, stock_on_hand, counted, unit_cost, \
                    CAST(COALESCE(variance_units,0) AS REAL), CAST(COALESCE(variance_cost,0) AS REAL) \
             FROM stocktake_run_lines WHERE run_id IN (SELECT id FROM stocktake_runs) \
             ORDER BY run_id, upc",
        )
        .fetch_all(pool)
        .await?;
    let mut by_run: std::collections::HashMap<String, Vec<StocktakeRunLine>> =
        std::collections::HashMap::new();
    for (run_id, upc, description, soh, counted, uc, vu, vc) in line_rows {
        by_run.entry(run_id).or_default().push(StocktakeRunLine {
            upc,
            description,
            stock_on_hand: soh,
            counted,
            unit_cost: uc,
            variance_units: vu,
            variance_cost: vc,
        });
    }
    Ok(rows
        .into_iter()
        .map(|(id, s, c, st, b, cf, tf, shrink, overage)| StocktakeRow {
            shrink_total: shrink,
            overage_total: overage,
            lines: by_run.remove(&id).unwrap_or_default(),
            id: id.clone(),
            started_at: s,
            completed_at: c,
            status: st,
            branch_id: b,
            count_file: cf,
            ticket_file: tf,
        })
        .collect())
}

// ── Payment Mix + Hourly Curve (header-level rollups, migration 0013) ────────

#[derive(Debug, serde::Serialize, Clone)]
pub struct PaymentRow {
    pub media: String,
    pub txns: i64,
    pub value: f64,
    pub fees: f64,
    pub change_amt: f64,
}

pub async fn payment_mix(pool: &SqlitePool, from: &str, to: &str, branch: Option<i64>) -> anyhow::Result<Vec<PaymentRow>> {
    let rows: Vec<(String, i64, f64, f64, f64)> = if let Some(b) = branch {
        sqlx::query_as(
            "SELECT media, SUM(txns), CAST(SUM(value) AS REAL), CAST(SUM(fees) AS REAL), CAST(SUM(change_amt) AS REAL) \
             FROM sales_payment WHERE branch_id = ?1 AND sale_date >= ?2 AND sale_date <= ?3 \
             GROUP BY media ORDER BY SUM(value) DESC",
        )
        .bind(b).bind(from).bind(to)
        .fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT media, SUM(txns), CAST(SUM(value) AS REAL), CAST(SUM(fees) AS REAL), CAST(SUM(change_amt) AS REAL) \
             FROM sales_payment WHERE sale_date >= ?1 AND sale_date <= ?2 \
             GROUP BY media ORDER BY SUM(value) DESC",
        )
        .bind(from).bind(to)
        .fetch_all(pool).await?
    };
    Ok(rows.into_iter().map(|(media, txns, value, fees, change_amt)| PaymentRow { media, txns, value, fees, change_amt }).collect())
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct HourlyRow {
    pub hour: i32,
    pub txns: i64,
    pub net: f64,
    pub stations: i64,
}

pub async fn hourly_curve(pool: &SqlitePool, from: &str, to: &str, branch: Option<i64>) -> anyhow::Result<Vec<HourlyRow>> {
    let rows: Vec<(i32, i64, f64, i64)> = if let Some(b) = branch {
        sqlx::query_as(
            "SELECT hour, SUM(txns), CAST(SUM(net) AS REAL), COUNT(DISTINCT station) \
             FROM sales_hourly WHERE branch_id = ?1 AND sale_date >= ?2 AND sale_date <= ?3 \
             GROUP BY hour ORDER BY hour",
        )
        .bind(b).bind(from).bind(to)
        .fetch_all(pool).await?
    } else {
        sqlx::query_as(
            "SELECT hour, SUM(txns), CAST(SUM(net) AS REAL), COUNT(DISTINCT station) \
             FROM sales_hourly WHERE sale_date >= ?1 AND sale_date <= ?2 \
             GROUP BY hour ORDER BY hour",
        )
        .bind(from).bind(to)
        .fetch_all(pool).await?
    };
    Ok(rows.into_iter().map(|(hour, txns, net, stations)| HourlyRow { hour, txns, net, stations }).collect())
}
