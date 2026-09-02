// Ingest engine — pull from a Connector into the local SQLite DB.
// Resumable: every table's position lives in `high_watermarks`, so an
// interrupted seed/poll continues where it left off (O-4).

use anyhow::Context;
use sqlx::sqlite::SqlitePool;

use crate::connector::{Connector, HighWater};
use crate::connector::{LiveDepartment, LiveSupplier};

pub const SOURCE_SEED: &str = "seed";
pub const BATCH: i64 = 5000;

/// Full reference pull (branches, departments, suppliers) — small tables,
/// always pulled wholesale. Writes provenance rows + maps ext ids.
pub async fn ingest_reference<C: Connector + ?Sized>(
    pool: &SqlitePool,
    conn: &C,
    source: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    // Branches (ext_key = AKPOS ID; local id = same id for the primary HoS DB)
    for b in conn.pull_branches().await? {
        sqlx::query(
            "INSERT INTO branches (id, ext_key, source, source_key, name, short_name, \
                    address, city, region, postcode, country, phone, gst_no, is_ho, last_synced_at) \
             VALUES (?1, ?2, ?3, ?2, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET \
               ext_key=excluded.ext_key, source=excluded.source, source_key=excluded.source_key, \
               name=excluded.name, short_name=excluded.short_name, address=excluded.address, \
               city=excluded.city, region=excluded.region, postcode=excluded.postcode, \
               country=excluded.country, phone=excluded.phone, gst_no=excluded.gst_no, \
               is_ho=excluded.is_ho, last_synced_at=excluded.last_synced_at",
        )
        .bind(b.id)
        .bind(b.id)
        .bind(source)
        .bind(&b.name)
        .bind(&b.short_name)
        .bind(&b.address)
        .bind(&b.city)
        .bind(&b.region)
        .bind(&b.postcode)
        .bind(&b.country)
        .bind(&b.phone)
        .bind(&b.gst_no)
        .bind(b.is_ho)
        .execute(&mut *tx)
        .await?;
    }
    let n = count(&mut tx, "branches").await?;
    tracing::info!("branches: {n} rows");

    for d in conn.pull_departments().await? {
        upsert_department(&mut tx, source, &d).await?;
    }
    let n = count(&mut tx, "departments").await?;
    tracing::info!("departments: {n} rows");

    for s in conn.pull_suppliers().await? {
        upsert_supplier(&mut tx, source, &s).await?;
    }
    let n = count(&mut tx, "suppliers").await?;
    tracing::info!("suppliers: {n} rows");

    tx.commit().await?;
    Ok(())
}

/// Incremental pull for one table. Reads its high-water mark, pulls batches
/// until the source is exhausted, committing each batch (resumable).
pub async fn ingest_items<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let mut hw = load_hw(pool, source, "items").await?;
    let mut total: u64 = 0;
    loop {
        let batch = conn.pull_items(&hw, BATCH).await?;
        let n = batch.rows.len();
        if n == 0 { break; }
        let mut tx = pool.begin().await?;
        for it in &batch.rows {
            upsert_item(&mut tx, source, it).await?;
        }
        // persist high-water inside the same txn as the batch
        if let Some(k) = &batch.next_key {
            set_hw(&mut tx, source, "items", k).await?;
        }
        tx.commit().await?;
        total += n as u64;
        tracing::info!("items: +{n} (total {total}, hw {})", batch.next_key.as_deref().unwrap_or("-"));
        if n < BATCH as usize { break; }
        hw.last_key = batch.next_key;
    }
    Ok(total)
}

pub async fn ingest_stock<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    // stock_current is rebuilt wholesale per branch on every poll (fresh
    // on-hand = latest movement per UPC). No high-water needed.
    let branch_ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM branches WHERE is_active = 1")
        .fetch_all(pool).await?;
    let mut total: u64 = 0;
    let mut tx = pool.begin().await?;
    for b in branch_ids {
        let rows = conn.pull_stock(b).await?;
        let n = rows.len();
        for s in &rows {
            sqlx::query(
                "INSERT INTO stock_current (branch_id, upc, qty, as_of, source) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(branch_id, upc) DO UPDATE SET \
                   qty=excluded.qty, as_of=excluded.as_of, source=excluded.source",
            )
            .bind(s.branch_id).bind(&s.upc).bind(s.qty).bind(&s.as_of).bind(source)
            .execute(&mut *tx).await?;
            total += 1;
        }
        tracing::info!("stock branch {b}: {n} rows");
    }
    tx.commit().await?;
    Ok(total)
}

pub async fn ingest_sales<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let mut hw = load_hw(pool, source, "sales").await?;
    let mut total: u64 = 0;
    loop {
        let batch = conn.pull_sales(&hw, BATCH).await?;
        let n = batch.rows.len();
        if n == 0 { break; }
        let mut tx = pool.begin().await?;
        for s in &batch.rows {
            // aggregate into sales_daily (branch, upc, date)
            let promo = if s.line_type == "S" { s.units } else { 0.0 };
            let normal = if s.line_type == "N" { s.units } else { 0.0 };
            sqlx::query(
                "INSERT INTO sales_daily (branch_id, upc, sale_date, units, revenue, \
                        promo_units, normal_units, promo_price, cost_amount, line_margin) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(branch_id, upc, sale_date) DO UPDATE SET \
                   units = units + excluded.units, \
                   revenue = revenue + excluded.revenue, \
                   promo_units = promo_units + excluded.promo_units, \
                   normal_units = normal_units + excluded.normal_units, \
                   cost_amount = cost_amount + excluded.cost_amount, \
                   line_margin = line_margin + excluded.line_margin",
            )
            .bind(s.branch_id).bind(&s.upc).bind(&s.sale_date)
            .bind(s.units).bind(s.revenue)
            .bind(promo).bind(normal)
            .bind(s.revenue / if s.units != 0.0 { s.units } else { 1.0 })
            .bind(s.cost * s.units)
            .bind(s.revenue - s.cost * s.units)
            .execute(&mut *tx).await?;
        }
        if let Some(k) = &batch.next_key {
            set_hw(&mut tx, source, "sales", k).await?;
        }
        tx.commit().await?;
        total += n as u64;
        tracing::info!("sales: +{n} (total {total})");
        if n < BATCH as usize { break; }
        hw.last_key = batch.next_key;
    }
    Ok(total)
}

pub async fn ingest_receipts<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let mut hw = load_hw(pool, source, "receipts").await?;
    let mut total: u64 = 0;
    loop {
        let batch = conn.pull_receipts(&hw, BATCH).await?;
        let n = batch.rows.len();
        if n == 0 { break; }
        let mut tx = pool.begin().await?;
        for r in &batch.rows {
            let supplier_id: Option<i64> = sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
                .bind(r.supplier.as_deref().unwrap_or("")).fetch_optional(&mut *tx).await?;
            let receipt_id: i64 = sqlx::query(
                "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, \
                        invoice_no, total_cost, logged, bill_of_lading, originating_trans_no) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(branch_id, trans_no, station) DO UPDATE SET \
                   trans_type=excluded.trans_type, supplier_id=excluded.supplier_id, \
                   invoice_no=excluded.invoice_no, total_cost=excluded.total_cost, logged=excluded.logged, \
                   bill_of_lading=excluded.bill_of_lading, originating_trans_no=excluded.originating_trans_no",
            )
            .bind(r.branch_id).bind(r.trans_no).bind(r.station).bind(&r.trans_type)
            .bind(supplier_id).bind(&r.invoice_no).bind(r.total_cost).bind(&r.logged)
            .bind(&r.bill_of_lading).bind(r.originating_trans_no)
            .execute(&mut *tx).await?
            .last_insert_rowid();
            // replace detail lines (receipts are append-only in practice)
            sqlx::query("DELETE FROM receipt_lines WHERE receipt_id = ?1").bind(receipt_id).execute(&mut *tx).await?;
            for l in &r.lines {
                sqlx::query(
                    "INSERT INTO receipt_lines (receipt_id, upc, quantity, unit_cost, ext_cost, status, cost_ave_local) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .bind(receipt_id).bind(&l.upc).bind(l.quantity).bind(l.unit_cost)
                .bind(l.ext_cost).bind(&l.status).bind(l.cost_ave_local)
                .execute(&mut *tx).await?;
            }
        }
        if let Some(k) = &batch.next_key {
            set_hw(&mut tx, source, "receipts", k).await?;
        }
        tx.commit().await?;
        total += n as u64;
        tracing::info!("receipts: +{n} (total {total})");
        if n < BATCH as usize { break; }
        hw.last_key = batch.next_key;
    }
    Ok(total)
}

pub async fn ingest_ap<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let mut hw = load_hw(pool, source, "ap").await?;
    let mut total: u64 = 0;
    loop {
        let batch = conn.pull_ap(&hw, BATCH).await?;
        let n = batch.rows.len();
        if n == 0 { break; }
        let mut tx = pool.begin().await?;
        for a in &batch.rows {
            let supplier_id: Option<i64> = sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
                .bind(a.supplier_code.as_deref().unwrap_or("")).fetch_optional(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO ap_invoices (branch_id, supplier_id, invoice_number, description, \
                        invoice_date, due_date, discount_date, invoice_amount, paid_amount, \
                        discount_amount, discount_pc, po_number, freight, tax_amount1, status, \
                        is_matched, logged) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
                 ON CONFLICT(branch_id, supplier_id, invoice_number) DO UPDATE SET \
                   description=excluded.description, invoice_date=excluded.invoice_date, \
                   due_date=excluded.due_date, discount_date=excluded.discount_date, \
                   invoice_amount=excluded.invoice_amount, paid_amount=excluded.paid_amount, \
                   discount_amount=excluded.discount_amount, discount_pc=excluded.discount_pc, \
                   po_number=excluded.po_number, freight=excluded.freight, \
                   tax_amount1=excluded.tax_amount1, status=excluded.status, \
                   is_matched=excluded.is_matched, logged=excluded.logged",
            )
            .bind(a.branch_id).bind(supplier_id).bind(&a.invoice_number).bind(&a.description)
            .bind(&a.invoice_date).bind(&a.due_date).bind(&a.discount_date)
            .bind(a.invoice_amount).bind(a.paid_amount).bind(a.discount_amount)
            .bind(a.discount_pc).bind(&a.po_number).bind(a.freight).bind(a.tax_amount1)
            .bind(&a.status).bind(a.is_matched).bind(&a.logged)
            .execute(&mut *tx).await?;
        }
        if let Some(k) = &batch.next_key {
            set_hw(&mut tx, source, "ap", k).await?;
        }
        tx.commit().await?;
        total += n as u64;
        tracing::info!("ap: +{n} (total {total})");
        if n < BATCH as usize { break; }
        hw.last_key = batch.next_key;
    }
    Ok(total)
}

pub async fn ingest_promos<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let promos = conn.pull_promos().await?;
    let mut tx = pool.begin().await?;
    for p in &promos {
        sqlx::query(
            "INSERT INTO promo_rules (kind, source, source_key, description, payload, sequence_match, \
                    condition_type, adjustment_type, adjustment_value, effective_start, \
                    effective_end, branch_scope, is_active, last_synced_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, datetime('now')) \
             ON CONFLICT(kind, source, source_key) DO UPDATE SET \
               description=excluded.description, payload=excluded.payload, \
               sequence_match=excluded.sequence_match, condition_type=excluded.condition_type, \
               adjustment_type=excluded.adjustment_type, adjustment_value=excluded.adjustment_value, \
               effective_start=excluded.effective_start, effective_end=excluded.effective_end, \
               branch_scope=excluded.branch_scope, is_active=excluded.is_active, \
               last_synced_at=excluded.last_synced_at",
        )
        .bind(&p.kind).bind(source).bind(&p.source_key).bind(&p.description).bind(&p.payload)
        .bind(&p.sequence_match).bind(&p.condition_type).bind(&p.adjustment_type)
        .bind(p.adjustment_value).bind(&p.effective_start).bind(&p.effective_end)
        .bind(p.branch_scope).bind(!p.inactive)
        .execute(&mut *tx).await?;
    }
    let n = promos.len();
    tx.commit().await?;
    tracing::info!("promos: {n} rows");
    Ok(n as u64)
}

/// RBP set/group materialization (wipe-reload — small reference tables;
/// upsert would leave stale rows behind).
pub async fn ingest_rbp<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<u64> {
    let groups = conn.pull_pricing_groups().await?;
    let sets = conn.pull_pricing_sets().await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM pricing_groups").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM pricing_sets").execute(&mut *tx).await?;
    for g in &groups {
        sqlx::query(
            "INSERT INTO pricing_groups (group_id, description, data_key, type, is_active) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(g.group_id).bind(&g.description).bind(&g.data_key).bind(&g.type_).bind(g.is_active)
        .execute(&mut *tx).await?;
    }
    for s in &sets {
        sqlx::query(
            "INSERT INTO pricing_sets (set_id, set_line, group_id, min_qty, max_qty) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(s.set_id).bind(s.set_line).bind(s.group_id).bind(s.min_qty).bind(s.max_qty)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    tracing::info!("rbp: {} groups, {} sets (source {source})", groups.len(), sets.len());
    Ok((groups.len() + sets.len()) as u64)
}

// ── helpers ──────────────────────────────────────────────────────────────────

async fn upsert_department(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, source: &str, d: &LiveDepartment) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO departments (id, ext_key, source, source_key, name, target_margin, last_synced_at) \
         VALUES (?1, ?1, ?2, ?1, ?3, ?4, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET source=excluded.source, source_key=excluded.source_key, \
           name=excluded.name, target_margin=excluded.target_margin, last_synced_at=excluded.last_synced_at",
    )
    .bind(d.id).bind(source).bind(&d.name).bind(d.target_margin)
    .execute(&mut **tx).await?;
    Ok(())
}

async fn upsert_supplier(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, source: &str, s: &LiveSupplier) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO suppliers (ext_key, source, source_key, code, name, first_name, last_name, \
                disc_group, disc_percent, disc_days, last_synced_at) \
         VALUES (?1, ?2, ?1, ?1, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now')) \
         ON CONFLICT(code) DO UPDATE SET source=excluded.source, source_key=excluded.source_key, \
           name=excluded.name, first_name=excluded.first_name, last_name=excluded.last_name, \
           disc_group=excluded.disc_group, disc_percent=excluded.disc_percent, \
           disc_days=excluded.disc_days, last_synced_at=excluded.last_synced_at",
    )
    .bind(&s.code).bind(source).bind(&s.last_name).bind(&s.first_name)
    .bind(s.disc_group).bind(s.disc_percent).bind(s.disc_days)
    .execute(&mut **tx).await?;
    Ok(())
}

async fn upsert_item(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, source: &str, it: &crate::connector::LiveItem) -> anyhow::Result<()> {
    let dept_id: Option<i64> = sqlx::query_scalar("SELECT id FROM departments WHERE ext_key = ?1")
        .bind(it.department).fetch_optional(&mut **tx).await?;
    let sup_id: Option<i64> = sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
        .bind(it.supplier.as_deref().unwrap_or("")).fetch_optional(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO items (upc, source, source_key, sku, description, department_id, supplier_id, \
                parent_upc, class, sub_department, cost, cost_ave, purchase_cost, \
                price1, price2, price3, price4, price5, price6, price7, price8, \
                tax_code, pack_units, volume_ml, non_stock, is_active, last_synced_at) \
         VALUES (?1, ?2, ?1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, datetime('now')) \
         ON CONFLICT(upc) DO UPDATE SET \
           source=excluded.source, source_key=excluded.source_key, sku=excluded.sku, \
           description=excluded.description, department_id=excluded.department_id, \
           supplier_id=excluded.supplier_id, parent_upc=excluded.parent_upc, \
           class=excluded.class, sub_department=excluded.sub_department, \
           cost=excluded.cost, cost_ave=excluded.cost_ave, purchase_cost=excluded.purchase_cost, \
           price1=excluded.price1, price2=excluded.price2, price3=excluded.price3, \
           price4=excluded.price4, price5=excluded.price5, price6=excluded.price6, \
           price7=excluded.price7, price8=excluded.price8, tax_code=excluded.tax_code, \
           pack_units=excluded.pack_units, volume_ml=excluded.volume_ml, \
           non_stock=excluded.non_stock, is_active=excluded.is_active, last_synced_at=excluded.last_synced_at",
    )
    .bind(&it.upc).bind(source).bind(&it.sku).bind(&it.description)
    .bind(dept_id).bind(sup_id).bind(&it.parent_upc)
    .bind(it.class).bind(it.sub_department)
    .bind(it.cost).bind(it.cost_ave).bind(it.purchase_cost)
    .bind(it.price1).bind(it.price2).bind(it.price3).bind(it.price4)
    .bind(it.price5).bind(it.price6).bind(it.price7).bind(it.price8)
    .bind(it.tax_no).bind(it.pack_units).bind(it.volume_ml)
    .bind(it.non_stock).bind(!it.inactive)
    .execute(&mut **tx).await?;
    Ok(())
}

async fn load_hw(pool: &SqlitePool, source: &str, table: &str) -> anyhow::Result<HighWater> {
    let last: Option<String> = sqlx::query_scalar("SELECT last_key FROM high_watermarks WHERE source = ?1 AND table_name = ?2")
        .bind(source).bind(table).fetch_optional(pool).await?;
    Ok(HighWater { source: source.into(), table: table.into(), last_key: last })
}

async fn set_hw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, source: &str, table: &str, key: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO high_watermarks (source, table_name, last_key, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now')) \
         ON CONFLICT(source, table_name) DO UPDATE SET last_key=excluded.last_key, updated_at=excluded.updated_at",
    )
    .bind(source).bind(table).bind(key)
    .execute(&mut **tx).await?;
    Ok(())
}

async fn count(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, table: &str) -> anyhow::Result<i64> {
    // sqlx 0.9 rejects dynamic SQL in query() — dispatch on the three known
    // reference tables (internal constants only, never user input).
    let n = match table {
        "branches" => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM branches").fetch_one(&mut **tx).await?,
        "departments" => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM departments").fetch_one(&mut **tx).await?,
        "suppliers" => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM suppliers").fetch_one(&mut **tx).await?,
        other => anyhow::bail!("count: unknown table {other}"),
    };
    Ok(n)
}

/// Run a full seed from a connector into the app DB.
pub async fn run_seed<C: Connector + ?Sized>(pool: &SqlitePool, conn: &C, source: &str) -> anyhow::Result<()> {
    tracing::info!("=== seed start (source={source}) ===");
    let probe = conn.probe().await.context("probe failed — is the live system reachable?")?;
    tracing::info!("probe: engine={} branches={:?}", probe.engine, probe.branch_ids);

    ingest_reference(pool, conn, source).await?;
    let items = ingest_items(pool, conn, source).await?;
    // Reset the items high-water to Updated-mode ("|" = ts mode from start) so
    // incremental polls catch EDITS to existing UPCs, not just new ones. The
    // first poll re-pulls the catalog once (idempotent upserts), then settles.
    sqlx::query(
        "INSERT INTO high_watermarks (source, table_name, last_key, updated_at) \
         VALUES (?1, 'items', '|', datetime('now')) \
         ON CONFLICT(source, table_name) DO UPDATE SET last_key='|', updated_at=datetime('now')",
    )
    .bind(source)
    .execute(pool)
    .await?;
    let stock = ingest_stock(pool, conn, source).await?;
    let sales = ingest_sales(pool, conn, source).await?;
    let receipts = ingest_receipts(pool, conn, source).await?;
    let ap = ingest_ap(pool, conn, source).await?;
    let promos = ingest_promos(pool, conn, source).await?;
    let rbp = ingest_rbp(pool, conn, source).await?;

    tracing::info!("=== seed complete: items={items} stock={stock} sales={sales} receipts={receipts} ap={ap} promos={promos} rbp={rbp} ===");
    Ok(())
}
