// Incoming-PO lifecycle (G-7 auto-flip) over the local SQLite DB.
//
// Status model (W5): waiting_import → pending_receipt → receipted
//   waiting_import : ETL .xlsx written to output/incoming-po/<branch>/,
//                    not yet imported into Infinity
//   pending_receipt: a 'P' purchase receipt appeared in the connector pull
//                    whose POID matches this PO (import confirmed)
//   receipted      : a 'G' goods-in appeared whose OriginatingTransNo points
//                    at that 'P' (stock receipted)
//
// The flip runs on every connector tick AFTER ingest_receipts, so it is
// purely local and works in every mode.
use sqlx::sqlite::SqlitePool;

/// Auto-flip incoming_pos statuses from the materialized receipts. Returns
/// the number of rows whose status changed.
pub async fn auto_flip(pool: &SqlitePool) -> anyhow::Result<u64> {
    let mut tx = pool.begin().await?;

    // 1. waiting_import → pending_receipt: a 'P' receipt carries our
    //    BillOfLading code (the ETL xlsx writes it; Infinity stores it on
    //    SMHeaders.BillOfLading). Import IS confirmed → capture the Infinity
    //    TransNo the PO became (transID) for the receipted link.
    let changed = sqlx::query(
        "UPDATE incoming_pos SET status = 'pending_receipt', imported = 1, imported_at = datetime('now'), \
                trans_no = (SELECT r.trans_no FROM receipts r \
                            WHERE r.trans_type = 'P' AND r.branch_id = incoming_pos.branch_id \
                              AND r.bill_of_lading = incoming_pos.bill_of_lading LIMIT 1) \
         WHERE status = 'waiting_import' AND EXISTS (\
             SELECT 1 FROM receipts r \
             WHERE r.trans_type = 'P' AND r.branch_id = incoming_pos.branch_id \
               AND r.bill_of_lading = incoming_pos.bill_of_lading)",
    )
    .execute(&mut *tx)
    .await?;
    let mut total = changed.rows_affected();

    // 2. pending_receipt → receipted: the captured transID (Infinity TransNo)
    //    is the OriginatingTransNo on a linked 'G' goods-in.
    let changed = sqlx::query(
        "UPDATE incoming_pos SET status = 'receipted', receipted_at = datetime('now') \
         WHERE status = 'pending_receipt' AND trans_no IS NOT NULL AND EXISTS (\
             SELECT 1 FROM receipts rg \
             WHERE rg.trans_type = 'G' AND rg.branch_id = incoming_pos.branch_id \
               AND rg.originating_trans_no = incoming_pos.trans_no)",
    )
    .execute(&mut *tx)
    .await?;
    total += changed.rows_affected();

    tx.commit().await?;
    if total > 0 {
        tracing::info!("incoming-po: auto-flipped {total} row(s)");
    }
    Ok(total)
}

/// List incoming POs with resolved supplier names.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows: Vec<(i64, Option<String>, i64, String, String, String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT ip.branch_id, s.code, ip.poid, ip.filename, ip.status, ip.placed_at, ip.imported_at, ip.trans_no \
         FROM incoming_pos ip LEFT JOIN suppliers s ON s.id = ip.supplier_id \
         ORDER BY ip.placed_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(branch_id, supplier_code, poid, filename, status, placed_at, imported_at, trans_no)| {
                serde_json::json!({
                    "branch_id": branch_id,
                    "supplier_code": supplier_code.unwrap_or_default(),
                    "poid": poid,
                    "filename": filename,
                    "status": status,
                    "placed_at": placed_at,
                    "imported_at": imported_at,
                    "trans_no": trans_no,
                })
            },
        )
        .collect())
}

/// Mark a PO deleted (HoS only — server-side enforced by the handler).
pub async fn remove(pool: &SqlitePool, filename: &str) -> anyhow::Result<u64> {
    let res = sqlx::query("DELETE FROM incoming_pos WHERE filename = ?1")
        .bind(filename)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
