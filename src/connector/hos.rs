// New-gen HoS connector (gg-core-hos, Rules_Based engine, RBP pricing).
// All queries port from the WebRMS module DB layers (verified live 2026-08).
// Cell reads use safe try_get helpers — AKPOS mixes smallint/int/money/decimal
// and tiberius get() panics on type mismatch (WebRMS cell_to_* lesson).

use super::*;
use futures_util::StreamExt;
use tiberius::{Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

type TcpStreamCompat = Compat<TcpStream>;

pub struct HosConnector {
    conn_string: String,
}

impl HosConnector {
    pub fn new(conn_string: String) -> Self {
        Self { conn_string }
    }

    async fn connect(&self) -> anyhow::Result<Client<TcpStreamCompat>> {
        let (cfg, host_port) = parse_conn_string(&self.conn_string)?;
        let tcp = TcpStream::connect(&host_port).await?;
        Ok(Client::connect(cfg, tcp.compat_write()).await?)
    }
}

/// Parse the key=value connection string into a tiberius Config + host:port.
pub fn parse_conn_string(s: &str) -> anyhow::Result<(Config, String)> {
    let mut host = "localhost".to_string();
    let mut port = 1433;
    let mut database = "AKPOS".to_string();
    let mut user = String::new();
    let mut pwd = String::new();
    for part in s.split(';') {
        let Some((k, v)) = part.split_once('=') else { continue };
        let v = v.trim();
        match k.trim().to_lowercase().as_str() {
            "server" | "host" => {
                let hostport = v.trim_start_matches("tcp:");
                if let Some((h, p)) = hostport.rsplit_once(',') {
                    host = h.to_string();
                    port = p.parse().unwrap_or(1433);
                } else {
                    host = hostport.to_string();
                }
            }
            "port" => port = v.parse().unwrap_or(1433),
            "database" | "db" => database = v.to_string(),
            "uid" | "user" => user = v.to_string(),
            "pwd" | "password" => pwd = v.to_string(),
            _ => {}
        }
    }
    let mut cfg = Config::new();
    cfg.host(&host);
    cfg.port(port);
    cfg.database(&database);
    cfg.authentication(tiberius::AuthMethod::sql_server(user, pwd));
    cfg.trust_cert(); // internal LAN; read-only viewer login
    Ok((cfg, format!("{host}:{port}")))
}

async fn query_rows(client: &mut Client<TcpStreamCompat>, sql: &str) -> anyhow::Result<Vec<tiberius::Row>> {
    let stream = client.query(sql, &[]).await?;
    let mut rows = Vec::new();
    let mut stream = stream.into_row_stream();
    while let Some(row) = stream.next().await {
        rows.push(row?);
    }
    Ok(rows)
}

async fn query_string(client: &mut Client<TcpStreamCompat>, sql: &str) -> anyhow::Result<String> {
    let rows = query_rows(client, sql).await?;
    Ok(rows.first().map(|r| gstr(r, 0)).unwrap_or_default())
}

// ── safe cell readers (tiberius get() panics on type mismatch; AKPOS mixes
//    smallint/int/money/decimal — mirror WebRMS cell_to_string/cell_to_f64) ──

/// i64 cell: accepts i32/i16/i64, falls back to 0.
fn gi(r: &tiberius::Row, i: usize) -> i64 {
    if let Ok(Some(v)) = r.try_get::<i64, _>(i) { return v; }
    if let Ok(Some(v)) = r.try_get::<i32, _>(i) { return v as i64; }
    if let Ok(Some(v)) = r.try_get::<i16, _>(i) { return v as i64; }
    0
}
/// i32 cell.
fn gi32(r: &tiberius::Row, i: usize) -> i32 {
    if let Ok(Some(v)) = r.try_get::<i32, _>(i) { return v; }
    if let Ok(Some(v)) = r.try_get::<i16, _>(i) { return v as i32; }
    if let Ok(Some(v)) = r.try_get::<i64, _>(i) { return v as i32; }
    0
}
/// f64 cell: accepts money/decimal/f64/f32/int.
fn gf(r: &tiberius::Row, i: usize) -> f64 {
    if let Ok(Some(v)) = r.try_get::<f64, _>(i) { return v; }
    if let Ok(Some(v)) = r.try_get::<i64, _>(i) { return v as f64; }
    if let Ok(Some(v)) = r.try_get::<i32, _>(i) { return v as f64; }
    0.0
}
/// String cell (None when NULL/empty).
fn gs(r: &tiberius::Row, i: usize) -> Option<String> {
    r.try_get::<&str, _>(i).ok().flatten().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
/// String cell, default "".
fn gstr(r: &tiberius::Row, i: usize) -> String {
    gs(r, i).unwrap_or_default()
}
/// bool cell.
fn gb(r: &tiberius::Row, i: usize) -> bool {
    if let Ok(Some(v)) = r.try_get::<bool, _>(i) { v }
    else if let Ok(Some(v)) = r.try_get::<i32, _>(i) { v != 0 }
    else { false }
}

fn sql_list(items: &[String]) -> String {
    items.iter().map(|u| format!("'{}'", u.replace('\'', "''"))).collect::<Vec<_>>().join(",")
}

impl Connector for HosConnector {
    async fn probe(&self) -> anyhow::Result<ProbeInfo> {
        let mut client = self.connect().await?;
        let engine = query_string(&mut client, "SELECT TOP 1 Value FROM Config WHERE ID = 6").await
            .unwrap_or_else(|_| "Standard".to_string());
        let branch_ids: Vec<i32> = query_rows(&mut client, "SELECT CAST(ID AS INT) FROM Branches ORDER BY ID").await?
            .iter().map(|r| gi32(r, 0)).collect();
        Ok(ProbeInfo { db_ok: true, engine, branch_ids })
    }

    async fn pull_branches(&self) -> anyhow::Result<Vec<LiveBranch>> {
        let mut client = self.connect().await?;
        let rows = query_rows(&mut client,
            "SELECT CAST(ID AS INT), IsHO, Name, HostName, Address1, City, State, PostCode, Country, Phone1, TaxNumber \
             FROM Branches ORDER BY ID").await?;
        Ok(rows.iter().map(|r| LiveBranch {
            id: gi(r, 0),
            is_ho: gb(r, 1),
            name: gstr(r, 2),
            short_name: gstr(r, 3),
            address: gs(r, 4),
            city: gs(r, 5),
            region: gs(r, 6),
            postcode: gs(r, 7),
            country: gs(r, 8),
            phone: gs(r, 9),
            gst_no: gs(r, 10),
        }).collect())
    }

    async fn pull_departments(&self) -> anyhow::Result<Vec<LiveDepartment>> {
        let mut client = self.connect().await?;
        let rows = query_rows(&mut client,
            "SELECT CAST(ID AS INT), Description, CAST(TargetMargin AS FLOAT) FROM Departments ORDER BY ID").await?;
        Ok(rows.iter().map(|r| LiveDepartment {
            id: gi(r, 0),
            name: gstr(r, 1),
            target_margin: gf(r, 2),
        }).collect())
    }

    async fn pull_suppliers(&self) -> anyhow::Result<Vec<LiveSupplier>> {
        let mut client = self.connect().await?;
        let rows = query_rows(&mut client,
            "SELECT Code, LastName, FirstName, CAST(DiscGroup AS INT), CAST(DiscPercent AS FLOAT), CAST(DiscDays AS INT) \
             FROM Customers WHERE CustType='R' AND (InActive = 0 OR InActive IS NULL) ORDER BY Code").await?;
        Ok(rows.iter().map(|r| LiveSupplier {
            code: gstr(r, 0),
            last_name: gstr(r, 1),
            first_name: gs(r, 2),
            disc_group: Some(gi32(r, 3)).filter(|v| *v != 0),
            disc_percent: Some(gf(r, 4)).filter(|v| *v != 0.0),
            disc_days: Some(gi32(r, 5)).filter(|v| *v != 0),
        }).collect())
    }

    async fn pull_items(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveItem>> {
        let mut client = self.connect().await?;
        // Two keyset modes:
        //  - hw = "UPC:<upc>"   → full-catalog seed: page by UPC (indexed PK, linear)
        //  - hw = "ts|upc"      → incremental poll: page by (Updated, UPC)
        let last = hw.last_key.as_deref().unwrap_or("");
        let (mode, since_ts, since_upc): (&str, String, String) = if let Some(u) = last.strip_prefix("UPC:") {
            ("upc", String::new(), u.to_string())
        } else if let Some((t, u)) = last.split_once('|') {
            ("ts", t.to_string(), u.to_string())
        } else {
            ("upc", String::new(), String::new())
        };
        let (where_clause, order_clause) = match mode {
            "upc" if since_upc.is_empty() => (String::new(), "ORDER BY UPC ASC".to_string()),
            "upc" => (
                format!("WHERE UPC > '{}'", since_upc),
                "ORDER BY UPC ASC".to_string(),
            ),
            _ => (
                format!(
                    "WHERE (CONVERT(varchar(23), Updated, 121) > '{}' OR \
                           (CONVERT(varchar(23), Updated, 121) = '{}' AND UPC > '{}'))",
                    since_ts, since_ts, since_upc
                ),
                "ORDER BY Updated ASC, UPC ASC".to_string(),
            ),
        };
        let sql = format!(
            "SELECT TOP {limit} UPC, SKU, Description, CAST(Department AS INT), CAST(SubDepartment AS INT), \
                    CAST(Class AS INT), Supplier, ParentUPC, CAST(Cost AS FLOAT), CAST(CostAve AS FLOAT), \
                    CAST(PurchaseCost AS FLOAT), CAST(Price1 AS FLOAT), CAST(Price2 AS FLOAT), \
                    CAST(Price3 AS FLOAT), CAST(Price4 AS FLOAT), CAST(Price5 AS FLOAT), \
                    CAST(Price6 AS FLOAT), CAST(Price7 AS FLOAT), CAST(Price8 AS FLOAT), \
                    CAST(TaxNo AS INT), CAST(PurchaseQty AS FLOAT), \
                    NonStock, InActive, CONVERT(varchar(23), Updated, 121) \
             FROM Items \
             {where_clause} \
             {order_clause}");
        let rows = query_rows(&mut client, &sql).await?;
        let mut next_key = None;
        let items = rows.iter().map(|r| {
            let upd = gs(r, 23).unwrap_or_default();
            let upc = gstr(r, 0);
            // next keyset depends on mode (UPC-only for seed, ts|upc for poll)
            next_key = Some(if mode == "upc" {
                format!("UPC:{upc}")
            } else {
                format!("{upd}|{upc}")
            });
            LiveItem {
                upc,
                sku: gstr(r, 1),
                description: gstr(r, 2),
                department: Some(gi32(r, 3)).filter(|v| *v != 0),
                sub_department: Some(gi32(r, 4)).filter(|v| *v != 0),
                class: Some(gi32(r, 5)).filter(|v| *v != 0),
                supplier: gs(r, 6),
                parent_upc: gs(r, 7),
                cost: gf(r, 8),
                cost_ave: gf(r, 9),
                purchase_cost: gf(r, 10),
                price1: gf(r, 11),
                price2: gf(r, 12),
                price3: gf(r, 13),
                price4: gf(r, 14),
                price5: gf(r, 15),
                price6: gf(r, 16),
                price7: gf(r, 17),
                price8: gf(r, 18),
                tax_no: Some(gi32(r, 19)).filter(|v| *v != 0),
                pack_units: gf(r, 20),
                volume_ml: None,
                non_stock: gb(r, 21),
                inactive: gb(r, 22),
                updated: Some(upd),
            }
        }).collect();
        Ok(PullResult { rows: items, next_key })
    }

    async fn pull_barcodes(&self, upcs: &[String]) -> anyhow::Result<Vec<LiveBarcode>> {
        if upcs.is_empty() { return Ok(vec![]); }
        let mut client = self.connect().await?;
        let sql = format!("SELECT UPC, Barcode FROM ItemBarCodes WHERE UPC IN ({})", sql_list(upcs));
        let rows = query_rows(&mut client, &sql).await?;
        Ok(rows.iter().map(|r| LiveBarcode {
            upc: gstr(r, 0),
            barcode: gstr(r, 1),
        }).collect())
    }

    async fn pull_stock(&self, branch_id: i32) -> anyhow::Result<Vec<LiveStock>> {
        let mut client = self.connect().await?;
        let sql = format!(
            "SELECT mv.UPC, CAST(mv.QtyOnHand + mv.Quantity AS FLOAT) AS Qty, CONVERT(varchar(23), mv.Logged, 121) AS AsOf \
             FROM ItemMovement mv \
             INNER JOIN (SELECT UPC, MAX(ID) AS MaxID FROM ItemMovement WHERE Branch = {branch_id} GROUP BY UPC) m \
               ON m.UPC = mv.UPC AND m.MaxID = mv.ID");
        let rows = query_rows(&mut client, &sql).await?;
        Ok(rows.iter().map(|r| LiveStock {
            branch_id,
            upc: gstr(r, 0),
            qty: gf(r, 1),
            as_of: gstr(r, 2),
        }).collect())
    }

    async fn pull_sales(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveSaleLine>> {
        let mut client = self.connect().await?;
        let since: i64 = hw.last_key.as_deref().unwrap_or("0").parse().unwrap_or(0);
        let sql = format!(
            "SELECT TOP {limit} mv.ID, CAST(mv.Branch AS INT), mv.UPC, CAST(mv.Quantity AS FLOAT), \
                    CAST(tl.Price AS FLOAT), CAST(tl.Cost AS FLOAT), tl.LineType, CONVERT(varchar(10), th.Logged, 120) \
             FROM ItemMovement mv \
             JOIN TransLines tl ON tl.UPC = mv.UPC AND tl.TransNo = mv.TransNo \
                                  AND tl.Branch = mv.Branch AND tl.Station = mv.Station \
             JOIN TransHeaders th ON th.TransNo = mv.TransNo AND th.Branch = mv.Branch AND th.Station = mv.Station \
             WHERE mv.TransType = 'T' AND mv.ID > {since} AND th.TransStatus = 'C' \
             ORDER BY mv.ID ASC");
        let rows = query_rows(&mut client, &sql).await?;
        let mut next_key = None;
        let sales = rows.iter().map(|r| {
            // keyset = LAST row's ID (advances the high-water mark)
            next_key = Some(gi(r, 0).to_string());
            LiveSaleLine {
                branch_id: gi32(r, 1),
                upc: gstr(r, 2),
                units: gf(r, 3),
                revenue: gf(r, 4),
                cost: gf(r, 5),
                line_type: gstr(r, 6),
                sale_date: gstr(r, 7),
            }
        }).collect();
        Ok(PullResult { rows: sales, next_key })
    }

    async fn pull_receipts(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveReceipt>> {
        let mut client = self.connect().await?;
        let since: i64 = hw.last_key.as_deref().unwrap_or("0").parse().unwrap_or(0);
        let sql = format!(
            "SELECT TOP {limit} sh.ID, CAST(sh.Branch AS INT), CAST(sh.TransNo AS INT), CAST(sh.Station AS INT), sh.TransType, \
                    sh.Supplier, sh.InvoiceNo, CAST(sh.TotalCost AS FLOAT), CONVERT(varchar(23), sh.Logged, 121) \
             FROM SMHeaders sh \
             WHERE sh.TransType IN ('P','G','I','Z') AND sh.ID > {since} \
             ORDER BY sh.ID ASC");
        let rows = query_rows(&mut client, &sql).await?;
        let mut next_key = None;
        let mut receipts = Vec::new();
        for r in &rows {
            let id = gi(r, 0);
            next_key = Some(id.to_string()); // keyset = LAST row's ID
            let branch = gi32(r, 1);
            let trans_no = gi(r, 2);
            let station = gi32(r, 3);
            let detail_sql = format!(
                "SELECT UPC, CAST(Quantity AS FLOAT), CAST(UnitCost AS FLOAT), CAST(ExtCost AS FLOAT), Status, CAST(CostAveLocal AS FLOAT) \
                 FROM SMDetails WHERE TransNo = {trans_no} AND Branch = {branch} AND Station = {station}");
            let lines = query_rows(&mut client, &detail_sql).await?;
            let parsed_lines = lines.iter().map(|d| LiveReceiptLine {
                upc: gstr(&d, 0),
                quantity: gf(&d, 1),
                unit_cost: gf(&d, 2),
                ext_cost: gf(&d, 3),
                status: gs(&d, 4),
                cost_ave_local: Some(gf(&d, 5)).filter(|v| *v != 0.0),
            }).collect();
            receipts.push(LiveReceipt {
                branch_id: branch,
                trans_no,
                station,
                trans_type: gstr(&r, 4),
                supplier: gs(&r, 5),
                invoice_no: gs(&r, 6),
                total_cost: gf(&r, 7),
                logged: gstr(&r, 8),
                lines: parsed_lines,
            });
        }
        Ok(PullResult { rows: receipts, next_key })
    }

    async fn pull_ap(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveApInvoice>> {
        let mut client = self.connect().await?;
        let since: i64 = hw.last_key.as_deref().unwrap_or("0").parse().unwrap_or(0);
        let sql = format!(
            "SELECT TOP {limit} ID, CAST(Branch AS INT), SupplierCode, InvoiceNumber, Description, \
                    CONVERT(varchar(10), InvoiceDate, 120), CONVERT(varchar(10), DueDate, 120), \
                    CONVERT(varchar(10), DiscountDate, 120), CAST(InvoiceAmount AS FLOAT), CAST(PaidAmount AS FLOAT), \
                    CAST(DiscountAmount AS FLOAT), CAST(DiscountPC AS FLOAT), PONumber, CAST(Freight AS FLOAT), \
                    CAST(TaxAmount1 AS FLOAT), Status, IsMatched, \
                    CONVERT(varchar(23), Logged, 121) \
             FROM APInv WHERE ID > {since} ORDER BY ID ASC");
        let rows = query_rows(&mut client, &sql).await?;
        let mut next_key = None;
        let ap = rows.iter().map(|r| {
            // keyset = LAST row's ID (advances the high-water mark)
            next_key = Some(gi(&r, 0).to_string());
            LiveApInvoice {
                branch_id: gi32(&r, 1),
                supplier_code: gs(&r, 2),
                invoice_number: gs(&r, 3),
                description: gs(&r, 4),
                invoice_date: gs(&r, 5),
                due_date: gs(&r, 6),
                discount_date: gs(&r, 7),
                invoice_amount: gf(&r, 8),
                paid_amount: gf(&r, 9),
                discount_amount: gf(&r, 10),
                discount_pc: Some(gf(&r, 11)).filter(|v| *v != 0.0),
                po_number: gs(&r, 12),
                freight: gf(&r, 13),
                tax_amount1: gf(&r, 14),
                status: gs(&r, 15),
                is_matched: gb(&r, 16),
                logged: gs(&r, 17),
            }
        }).collect();
        Ok(PullResult { rows: ap, next_key })
    }

    async fn pull_promos(&self) -> anyhow::Result<Vec<LivePromoRule>> {
        let mut client = self.connect().await?;
        let engine = query_string(&mut client, "SELECT TOP 1 Value FROM Config WHERE ID = 6").await
            .unwrap_or_else(|_| "Standard".to_string());
        if engine.trim().eq_ignore_ascii_case("Rules_Based") {
            let rows = query_rows(&mut client,
                "SELECT pc.ConditionID, ps.ConditionType, pc.SequenceMatch, pc.AdjustmentType, \
                        CAST(pc.AdjustmentValue AS FLOAT), CONVERT(varchar(10), pc.EffectiveStartDate, 120), \
                        CONVERT(varchar(10), pc.EffectiveEndDate, 120), CAST(pc.Branch AS INT), pc.InActive, \
                        pc.Description \
                 FROM PricingCondition pc \
                 JOIN PricingSequence ps ON ps.SequenceID = pc.SequenceID \
                 WHERE ISNULL(pc.InActive, 0) = 0").await?;
            Ok(rows.iter().map(|r| LivePromoRule {
                kind: "rbp_condition".into(),
                source_key: gi(&r, 0).to_string(),
                sequence_match: gs(&r, 2),
                condition_type: gs(&r, 1),
                adjustment_type: gs(&r, 3),
                adjustment_value: Some(gf(&r, 4)).filter(|v| *v != 0.0),
                effective_start: gs(&r, 5),
                effective_end: gs(&r, 6),
                branch_scope: Some(gi32(&r, 7)).filter(|v| *v != 0),
                inactive: gb(&r, 8),
                payload: format!("{{\"condition_id\":{}}}", gi(&r, 0)),
            }).collect())
        } else {
            let rows = query_rows(&mut client,
                "SELECT Product, ProductField, CONVERT(varchar(10), FromDate, 120), \
                        CONVERT(varchar(10), ToDate, 120), PriceType, CAST(Value AS FLOAT), CAST(Branch AS INT), InActive \
                 FROM Specials WHERE ISNULL(InActive, 0) = 0").await?;
            Ok(rows.iter().map(|r| LivePromoRule {
                kind: "special".into(),
                source_key: format!("{}|{}", gstr(&r, 0), gstr(&r, 1)),
                sequence_match: gs(&r, 0),
                condition_type: gs(&r, 1),
                adjustment_type: gs(&r, 4),
                adjustment_value: Some(gf(&r, 5)).filter(|v| *v != 0.0),
                effective_start: gs(&r, 2),
                effective_end: gs(&r, 3),
                branch_scope: Some(gi32(&r, 6)).filter(|v| *v != 0),
                inactive: gb(&r, 7),
                payload: String::new(),
            }).collect())
        }
    }
}
