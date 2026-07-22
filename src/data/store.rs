use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

use super::models::{normalize_code, Fundamental, KLine, Market, Period, Position, Stock, Trade};

pub struct Store {
    conn: Connection,
}

fn default_db_path() -> Result<PathBuf> {
    let dirs =
        directories::ProjectDirs::from("dev", "wnzzer", "stockrs").context("无法确定数据目录")?;
    let dir = dirs.data_dir().to_path_buf();
    std::fs::create_dir_all(&dir).context("无法创建数据目录")?;
    Ok(dir.join("stockrs.db"))
}

impl Store {
    pub fn open_default() -> Result<Store> {
        Store::open(default_db_path()?)
    }

    pub fn open(path: PathBuf) -> Result<Store> {
        let conn = Connection::open(&path)
            .with_context(|| format!("无法打开数据库 {}", path.display()))?;
        let store = Store { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        // klines 加入 klt 周期分区维度。旧库(无 klt 列)直接重建——项目无外部用户,
        // 不做数据迁移,老日K重新 data update 即可。
        let klines_exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='klines'",
            [],
            |r| r.get(0),
        )?;
        if klines_exists > 0 {
            let has_klt: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('klines') WHERE name = 'klt'",
                [],
                |r| r.get(0),
            )?;
            if has_klt == 0 {
                self.conn.execute("DROP TABLE klines", [])?;
            }
        }
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS stocks (
                code TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                market TEXT NOT NULL,
                added_at TEXT NOT NULL,
                lot_size INTEGER NOT NULL DEFAULT 100
            );
            CREATE TABLE IF NOT EXISTS klines (
                code TEXT NOT NULL,
                klt  TEXT NOT NULL,
                date TEXT NOT NULL,
                open REAL NOT NULL,
                high REAL NOT NULL,
                low REAL NOT NULL,
                close REAL NOT NULL,
                volume REAL NOT NULL,
                amount REAL NOT NULL,
                turnover REAL,
                PRIMARY KEY (code, klt, date)
            );
            CREATE TABLE IF NOT EXISTS portfolio (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                code TEXT NOT NULL,
                price REAL NOT NULL,
                quantity INTEGER NOT NULL,
                date TEXT NOT NULL,
                note TEXT
            );
            CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                code TEXT NOT NULL,
                action TEXT NOT NULL,
                price REAL NOT NULL,
                quantity INTEGER NOT NULL,
                date TEXT NOT NULL,
                note TEXT,
                cost_basis REAL,
                pnl REAL
            );
            CREATE TABLE IF NOT EXISTS fundamentals (
                code TEXT NOT NULL,
                date TEXT NOT NULL,
                pe_ttm REAL,
                pb_mrq REAL,
                ps_ttm REAL,
                total_mv REAL,
                PRIMARY KEY (code, date)
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        // 迁移:老库 stocks 表补 lot_size 列(SQLite 无 ADD COLUMN IF NOT EXISTS)。
        let has_lot: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('stocks') WHERE name = 'lot_size'",
            [],
            |r| r.get(0),
        )?;
        if has_lot == 0 {
            self.conn.execute(
                "ALTER TABLE stocks ADD COLUMN lot_size INTEGER NOT NULL DEFAULT 100",
                [],
            )?;
        }
        // 迁移:老库 trades 表补 cost_basis / pnl 列(卖出记录已实现盈亏)。
        for col in ["cost_basis", "pnl"] {
            let has: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('trades') WHERE name = ?1",
                params![col],
                |r| r.get(0),
            )?;
            if has == 0 {
                self.conn
                    .execute(&format!("ALTER TABLE trades ADD COLUMN {col} REAL"), [])?;
            }
        }
        Ok(())
    }

    // ---- stocks ----

    pub fn add_stock(&self, stock: &Stock) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stocks (code, name, market, added_at, lot_size)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                stock.code,
                stock.name,
                stock.market.as_str(),
                stock.added_at,
                stock.lot_size
            ],
        )?;
        Ok(())
    }

    pub fn remove_stock(&self, code: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM stocks WHERE code = ?1", params![code])?;
        self.conn
            .execute("DELETE FROM klines WHERE code = ?1", params![code])?;
        self.conn
            .execute("DELETE FROM fundamentals WHERE code = ?1", params![code])?;
        Ok(n > 0)
    }

    pub fn get_stock(&self, code: &str) -> Result<Option<Stock>> {
        let mut stmt = self
            .conn
            .prepare("SELECT code, name, market, added_at, lot_size FROM stocks WHERE code = ?1")?;
        let mut rows = stmt.query(params![code])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_stock(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_stocks(&self) -> Result<Vec<Stock>> {
        let mut stmt = self
            .conn
            .prepare("SELECT code, name, market, added_at, lot_size FROM stocks ORDER BY code")?;
        let rows = stmt.query_map([], |row| Ok(row_to_stock(row)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    // ---- klines ----

    pub fn upsert_klines(&mut self, klines: &[KLine], period: Period) -> Result<usize> {
        let klt = period.tag();
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO klines
                 (code, klt, date, open, high, low, close, volume, amount, turnover)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for k in klines {
                stmt.execute(params![
                    k.code, klt, k.date, k.open, k.high, k.low, k.close, k.volume, k.amount,
                    k.turnover
                ])?;
            }
        }
        tx.commit()?;
        Ok(klines.len())
    }

    /// 返回指定股票指定周期的K线，按日期升序。可选起止日期过滤（闭区间）。
    pub fn get_klines(
        &self,
        code: &str,
        period: Period,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Vec<KLine>> {
        let klt = period.tag();
        let mut sql = String::from(
            "SELECT code, date, open, high, low, close, volume, amount, turnover
             FROM klines WHERE code = ?1 AND klt = ?2",
        );
        if start.is_some() {
            sql.push_str(" AND date >= ?3");
        }
        if end.is_some() {
            sql.push_str(if start.is_some() {
                " AND date <= ?4"
            } else {
                " AND date <= ?3"
            });
        }
        sql.push_str(" ORDER BY date ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&code, &klt];
        if let Some(s) = start.as_ref() {
            binds.push(s);
        }
        if let Some(e) = end.as_ref() {
            binds.push(e);
        }
        let rows = stmt.query_map(binds.as_slice(), |row| Ok(row_to_kline(row)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// 某只股票某周期已存的最新日期，用于增量更新。
    pub fn latest_kline_date(&self, code: &str, period: Period) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT MAX(date) FROM klines WHERE code = ?1 AND klt = ?2")?;
        let date: Option<String> =
            stmt.query_row(params![code, period.tag()], |row| row.get(0))?;
        Ok(date)
    }

    pub fn kline_count(&self, code: &str, period: Period) -> Result<i64> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM klines WHERE code = ?1 AND klt = ?2")?;
        let n: i64 = stmt.query_row(params![code, period.tag()], |row| row.get(0))?;
        Ok(n)
    }

    pub fn kline_date_range(&self, code: &str, period: Period) -> Result<Option<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT MIN(date), MAX(date) FROM klines WHERE code = ?1 AND klt = ?2",
        )?;
        let range: (Option<String>, Option<String>) =
            stmt.query_row(params![code, period.tag()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        match range {
            (Some(a), Some(b)) => Ok(Some((a, b))),
            _ => Ok(None),
        }
    }

    // ---- fundamentals ----

    pub fn upsert_fundamentals(&mut self, rows: &[Fundamental]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO fundamentals
                 (code, date, pe_ttm, pb_mrq, ps_ttm, total_mv)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for f in rows {
                stmt.execute(params![
                    f.code, f.date, f.pe_ttm, f.pb_mrq, f.ps_ttm, f.total_mv
                ])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// 返回指定股票的基本面,按日期升序(PIT 对齐依赖升序)。可选起止日期过滤。
    pub fn get_fundamentals(
        &self,
        code: &str,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Vec<Fundamental>> {
        let mut sql = String::from(
            "SELECT code, date, pe_ttm, pb_mrq, ps_ttm, total_mv
             FROM fundamentals WHERE code = ?1",
        );
        if start.is_some() {
            sql.push_str(" AND date >= ?2");
        }
        if end.is_some() {
            sql.push_str(if start.is_some() {
                " AND date <= ?3"
            } else {
                " AND date <= ?2"
            });
        }
        sql.push_str(" ORDER BY date ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&code];
        if let Some(s) = start.as_ref() {
            binds.push(s);
        }
        if let Some(e) = end.as_ref() {
            binds.push(e);
        }
        let rows = stmt.query_map(binds.as_slice(), |row| Ok(row_to_fundamental(row)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    pub fn latest_fundamental_date(&self, code: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT MAX(date) FROM fundamentals WHERE code = ?1")?;
        let date: Option<String> = stmt.query_row(params![code], |row| row.get(0))?;
        Ok(date)
    }

    /// 最近一条基本面（ORDER BY date DESC）。用于 quote 在实时源缺 PE/PB 时本地兜底。
    pub fn latest_fundamental(&self, code: &str) -> Result<Option<Fundamental>> {
        let mut stmt = self.conn.prepare(
            "SELECT code, date, pe_ttm, pb_mrq, ps_ttm, total_mv
             FROM fundamentals WHERE code = ?1 ORDER BY date DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![code], |row| Ok(row_to_fundamental(row)))?;
        match rows.next() {
            Some(r) => Ok(Some(r??)),
            None => Ok(None),
        }
    }

    // ---- portfolio ----

    pub fn add_position(
        &self,
        code: &str,
        price: f64,
        quantity: i64,
        date: &str,
        note: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO portfolio (code, price, quantity, date, note) VALUES (?1,?2,?3,?4,?5)",
            params![code, price, quantity, date, note],
        )?;
        self.conn.execute(
            "INSERT INTO trades (code, action, price, quantity, date, note)
             VALUES (?1,'buy',?2,?3,?4,?5)",
            params![code, price, quantity, date, note],
        )?;
        Ok(())
    }

    /// 移除持仓(纠正误录):清空该代码的持仓并撤销其买入记录,保留已实现的卖出记录。
    /// add→remove 因此对账本无残留;若曾部分卖出,卖出记录(及其已实现盈亏)仍保留。
    pub fn remove_position(&mut self, code: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM portfolio WHERE code = ?1", params![code])?;
        tx.execute(
            "DELETE FROM trades WHERE code = ?1 AND action = 'buy'",
            params![code],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// 卖出(减仓/清仓):按 FIFO(先进先出)消耗各买入批次,已实现盈亏 = 卖出额 − 被卖股份的原始成本。
    /// 成本口径与削减口径一致(都消耗最早批次),因此对多批建仓,累计已实现盈亏 = 总卖出额 − 总买入成本。
    /// 单批建仓时 FIFO 成本即买入价,与加权平均一致。已实现盈亏不含手续费。
    /// 并写入一条 sell 交易(带 cost_basis / pnl);quantity 必须 >0 且不超过持仓总量;整笔在事务中完成。
    pub fn sell_position(
        &mut self,
        code: &str,
        price: f64,
        quantity: i64,
        date: &str,
        note: Option<&str>,
    ) -> Result<SellOutcome> {
        if quantity <= 0 {
            return Err(anyhow!("卖出数量必须为正,收到 {}", quantity));
        }
        let tx = self.conn.transaction()?;
        // 现有批次(FIFO:按建仓日、再按 id 升序)。
        let lots: Vec<(i64, f64, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT id, price, quantity FROM portfolio WHERE code = ?1 ORDER BY date ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![code], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, i64>(2)?))
            })?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        let total_qty: i64 = lots.iter().map(|(_, _, q)| q).sum();
        if total_qty == 0 {
            return Err(anyhow!("{} 无持仓", code));
        }
        if quantity > total_qty {
            return Err(anyhow!(
                "卖出 {} 股超过持仓 {} 股",
                quantity,
                total_qty
            ));
        }
        // FIFO 削减:按建仓先后消耗批次,并累计被卖股份的原始成本(成本口径 = 削减口径,保证守恒)。
        // 整批卖光则删除,部分卖出则更新剩余数量。
        let mut remaining_to_sell = quantity;
        let mut consumed_cost = 0.0;
        for (id, p, q) in &lots {
            if remaining_to_sell == 0 {
                break;
            }
            let take = (*q).min(remaining_to_sell);
            consumed_cost += p * take as f64;
            if *q <= remaining_to_sell {
                tx.execute("DELETE FROM portfolio WHERE id = ?1", params![id])?;
            } else {
                tx.execute(
                    "UPDATE portfolio SET quantity = ?1 WHERE id = ?2",
                    params![*q - remaining_to_sell, id],
                )?;
            }
            remaining_to_sell -= take;
        }
        let avg_cost = consumed_cost / quantity as f64;
        let realized_pnl = price * quantity as f64 - consumed_cost;

        tx.execute(
            "INSERT INTO trades (code, action, price, quantity, date, note, cost_basis, pnl)
             VALUES (?1,'sell',?2,?3,?4,?5,?6,?7)",
            params![code, price, quantity, date, note, avg_cost, realized_pnl],
        )?;
        tx.commit()?;

        Ok(SellOutcome {
            avg_cost,
            realized_pnl,
            sold_qty: quantity,
            remaining_qty: total_qty - quantity,
        })
    }

    // ---- meta (键值杂项:现金余额等) ----

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get(0)?)),
            None => Ok(None),
        }
    }

    /// 现金余额(手动维护,不随买卖自动增减)。未设置/损坏/非有限值均返回 None,
    /// 避免 NaN/Inf 污染仪表盘总资产。
    pub fn get_cash(&self) -> Result<Option<f64>> {
        Ok(self
            .get_meta("cash")?
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite()))
    }

    pub fn set_cash(&self, amount: f64) -> Result<()> {
        self.set_meta("cash", &amount.to_string())
    }

    /// 已实现盈亏合计。code=None 为全部,Some 为单只。
    pub fn realized_pnl(&self, code: Option<&str>) -> Result<f64> {
        let sum: f64 = match code {
            Some(c) => self.conn.query_row(
                "SELECT COALESCE(SUM(pnl),0) FROM trades WHERE action='sell' AND code=?1",
                params![c],
                |r| r.get(0),
            )?,
            None => self.conn.query_row(
                "SELECT COALESCE(SUM(pnl),0) FROM trades WHERE action='sell'",
                [],
                |r| r.get(0),
            )?,
        };
        Ok(sum)
    }

    pub fn list_positions(&self) -> Result<Vec<Position>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, code, price, quantity, date, note FROM portfolio ORDER BY date")?;
        let rows = stmt.query_map([], |row| {
            Ok(Position {
                id: row.get(0)?,
                code: row.get(1)?,
                price: row.get(2)?,
                quantity: row.get(3)?,
                date: row.get(4)?,
                note: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_trades(&self) -> Result<Vec<Trade>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, code, action, price, quantity, date, note, cost_basis, pnl
             FROM trades ORDER BY date, id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Trade {
                id: row.get(0)?,
                code: row.get(1)?,
                action: row.get(2)?,
                price: row.get(3)?,
                quantity: row.get(4)?,
                date: row.get(5)?,
                note: row.get(6)?,
                cost_basis: row.get(7)?,
                pnl: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// `sell_position` 的结算结果:结算成本、已实现盈亏、卖出量与剩余持仓。
#[derive(Debug, Clone)]
pub struct SellOutcome {
    pub avg_cost: f64,
    pub realized_pnl: f64,
    pub sold_qty: i64,
    pub remaining_qty: i64,
}

fn row_to_stock(row: &rusqlite::Row) -> Result<Stock> {
    let market_str: String = row.get(2)?;
    let market = Market::from_str(&market_str)
        .or_else(|| normalize_code(&row.get::<_, String>(0).unwrap_or_default()).map(|(_, m)| m))
        .context("未知市场")?;
    Ok(Stock {
        code: row.get(0)?,
        name: row.get(1)?,
        market,
        added_at: row.get(3)?,
        lot_size: row.get(4)?,
    })
}

fn row_to_kline(row: &rusqlite::Row) -> Result<KLine> {
    Ok(KLine {
        code: row.get(0)?,
        date: row.get(1)?,
        open: row.get(2)?,
        high: row.get(3)?,
        low: row.get(4)?,
        close: row.get(5)?,
        volume: row.get(6)?,
        amount: row.get(7)?,
        turnover: row.get(8)?,
    })
}

fn row_to_fundamental(row: &rusqlite::Row) -> Result<Fundamental> {
    Ok(Fundamental {
        code: row.get(0)?,
        date: row.get(1)?,
        pe_ttm: row.get(2)?,
        pb_mrq: row.get(3)?,
        ps_ttm: row.get(4)?,
        total_mv: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> Store {
        Store::open(PathBuf::from(":memory:")).unwrap()
    }

    fn held_qty(store: &Store, code: &str) -> i64 {
        store
            .list_positions()
            .unwrap()
            .into_iter()
            .filter(|p| p.code == code)
            .map(|p| p.quantity)
            .sum()
    }

    #[test]
    fn sell_partial_then_full() {
        let mut s = mem_store();
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap();

        let o = s.sell_position("600000", 12.0, 40, "2024-02-01", None).unwrap();
        assert!((o.avg_cost - 10.0).abs() < 1e-9);
        assert!((o.realized_pnl - 80.0).abs() < 1e-9); // (12-10)*40
        assert_eq!(o.sold_qty, 40);
        assert_eq!(o.remaining_qty, 60);
        assert_eq!(held_qty(&s, "600000"), 60);
        assert!((s.realized_pnl(Some("600000")).unwrap() - 80.0).abs() < 1e-9);

        // 卖光剩余,持仓清空,累计已实现盈亏 = 80 + 120。
        let o2 = s.sell_position("600000", 12.0, 60, "2024-03-01", None).unwrap();
        assert!((o2.realized_pnl - 120.0).abs() < 1e-9);
        assert_eq!(o2.remaining_qty, 0);
        assert_eq!(held_qty(&s, "600000"), 0);
        assert!((s.realized_pnl(None).unwrap() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn sell_over_holding_errors_and_leaves_state() {
        let mut s = mem_store();
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap();
        assert!(s.sell_position("600000", 12.0, 200, "2024-02-01", None).is_err());
        assert!(s.sell_position("000001", 12.0, 100, "2024-02-01", None).is_err()); // 无持仓
        assert!(s.sell_position("600000", 12.0, 0, "2024-02-01", None).is_err()); // 非正数
        // 事务回滚:持仓与已实现盈亏均未变化。
        assert_eq!(held_qty(&s, "600000"), 100);
        assert!((s.realized_pnl(None).unwrap()).abs() < 1e-9);
    }

    #[test]
    fn sell_fifo_cost_basis_across_lots() {
        let mut s = mem_store();
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap(); // 先建仓
        s.add_position("600000", 20.0, 100, "2024-01-02", None).unwrap(); // 后建仓
        // FIFO:卖 150 消耗 100@10 + 50@20 → 成本 2000,均价 13.333,已实现 = 18*150 - 2000 = 700。
        let o = s.sell_position("600000", 18.0, 150, "2024-02-01", None).unwrap();
        assert!((o.avg_cost - 2000.0 / 150.0).abs() < 1e-9);
        assert!((o.realized_pnl - 700.0).abs() < 1e-9);
        assert_eq!(o.remaining_qty, 50);
        // 先削光 2024-01-01 批(100),再削 2024-01-02 批 50 → 剩余为后一批 50 股 @20。
        let lots: Vec<_> = s
            .list_positions()
            .unwrap()
            .into_iter()
            .filter(|p| p.code == "600000")
            .collect();
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].quantity, 50);
        assert!((lots[0].price - 20.0).abs() < 1e-9);
    }

    #[test]
    fn sell_multilot_closed_conserves_pnl() {
        // 守恒:多批建仓全部卖出后,累计已实现盈亏 = 总卖出额 − 总买入成本。
        let mut s = mem_store();
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap();
        s.add_position("600000", 20.0, 100, "2024-01-02", None).unwrap();
        let o1 = s.sell_position("600000", 30.0, 100, "2024-02-01", None).unwrap();
        assert!((o1.realized_pnl - 2000.0).abs() < 1e-9); // 消耗 100@10:30*100 - 1000
        let o2 = s.sell_position("600000", 30.0, 100, "2024-02-02", None).unwrap();
        assert!((o2.realized_pnl - 1000.0).abs() < 1e-9); // 消耗 100@20:30*100 - 2000
        // 总卖出额 6000 − 总买入成本 3000 = 3000(旧的加权平均口径会错算成 2500)。
        assert!((s.realized_pnl(None).unwrap() - 3000.0).abs() < 1e-9);
        assert_eq!(held_qty(&s, "600000"), 0);
    }

    #[test]
    fn remove_purges_buys_keeps_sells() {
        let mut s = mem_store();
        // 纯误录:add 后 remove,账本无残留。
        s.add_position("000001", 5.0, 100, "2024-01-01", None).unwrap();
        assert!(s.remove_position("000001").unwrap());
        assert!(s.list_trades().unwrap().iter().all(|t| t.code != "000001"));
        // 部分卖出后再 remove:买入记录清除,卖出记录(及已实现盈亏)保留。
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap();
        s.sell_position("600000", 12.0, 40, "2024-02-01", None).unwrap();
        s.remove_position("600000").unwrap();
        let trades = s.list_trades().unwrap();
        assert!(trades.iter().all(|t| !(t.code == "600000" && t.action == "buy")));
        assert_eq!(
            trades.iter().filter(|t| t.code == "600000" && t.action == "sell").count(),
            1
        );
        assert!((s.realized_pnl(Some("600000")).unwrap() - 80.0).abs() < 1e-9);
        assert_eq!(held_qty(&s, "600000"), 0);
    }

    #[test]
    fn sell_records_trade_with_pnl() {
        let mut s = mem_store();
        s.add_position("600000", 10.0, 100, "2024-01-01", None).unwrap();
        s.sell_position("600000", 12.0, 40, "2024-02-01", Some("减仓")).unwrap();
        let trades = s.list_trades().unwrap();
        let buy = trades.iter().find(|t| t.action == "buy").unwrap();
        assert!(buy.pnl.is_none() && buy.cost_basis.is_none());
        let sell = trades.iter().find(|t| t.action == "sell").unwrap();
        assert_eq!(sell.quantity, 40);
        assert!((sell.pnl.unwrap() - 80.0).abs() < 1e-9);
        assert!((sell.cost_basis.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(sell.note.as_deref(), Some("减仓"));
    }
}
