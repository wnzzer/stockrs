use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

use super::models::{infer_market, KLine, Market, Position, Stock, Trade};

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
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS stocks (
                code TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                market TEXT NOT NULL,
                added_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS klines (
                code TEXT NOT NULL,
                date TEXT NOT NULL,
                open REAL NOT NULL,
                high REAL NOT NULL,
                low REAL NOT NULL,
                close REAL NOT NULL,
                volume REAL NOT NULL,
                amount REAL NOT NULL,
                turnover REAL,
                PRIMARY KEY (code, date)
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
                note TEXT
            );
            "#,
        )?;
        Ok(())
    }

    // ---- stocks ----

    pub fn add_stock(&self, stock: &Stock) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stocks (code, name, market, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                stock.code,
                stock.name,
                stock.market.as_str(),
                stock.added_at
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
        Ok(n > 0)
    }

    pub fn get_stock(&self, code: &str) -> Result<Option<Stock>> {
        let mut stmt = self
            .conn
            .prepare("SELECT code, name, market, added_at FROM stocks WHERE code = ?1")?;
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
            .prepare("SELECT code, name, market, added_at FROM stocks ORDER BY code")?;
        let rows = stmt.query_map([], |row| Ok(row_to_stock(row)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    // ---- klines ----

    pub fn upsert_klines(&mut self, klines: &[KLine]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO klines
                 (code, date, open, high, low, close, volume, amount, turnover)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for k in klines {
                stmt.execute(params![
                    k.code, k.date, k.open, k.high, k.low, k.close, k.volume, k.amount, k.turnover
                ])?;
            }
        }
        tx.commit()?;
        Ok(klines.len())
    }

    /// 返回指定股票的所有日K，按日期升序。可选起止日期过滤（闭区间）。
    pub fn get_klines(
        &self,
        code: &str,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Vec<KLine>> {
        let mut sql = String::from(
            "SELECT code, date, open, high, low, close, volume, amount, turnover
             FROM klines WHERE code = ?1",
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
        let rows = stmt.query_map(binds.as_slice(), |row| Ok(row_to_kline(row)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// 某只股票已存的最新日期，用于增量更新。
    pub fn latest_kline_date(&self, code: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT MAX(date) FROM klines WHERE code = ?1")?;
        let date: Option<String> = stmt.query_row(params![code], |row| row.get(0))?;
        Ok(date)
    }

    pub fn kline_count(&self, code: &str) -> Result<i64> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM klines WHERE code = ?1")?;
        let n: i64 = stmt.query_row(params![code], |row| row.get(0))?;
        Ok(n)
    }

    pub fn kline_date_range(&self, code: &str) -> Result<Option<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT MIN(date), MAX(date) FROM klines WHERE code = ?1")?;
        let range: (Option<String>, Option<String>) =
            stmt.query_row(params![code], |row| Ok((row.get(0)?, row.get(1)?)))?;
        match range {
            (Some(a), Some(b)) => Ok(Some((a, b))),
            _ => Ok(None),
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

    pub fn remove_position(&self, code: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM portfolio WHERE code = ?1", params![code])?;
        Ok(n > 0)
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
            "SELECT id, code, action, price, quantity, date, note FROM trades ORDER BY date",
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
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn row_to_stock(row: &rusqlite::Row) -> Result<Stock> {
    let market_str: String = row.get(2)?;
    let market = Market::from_str(&market_str)
        .or_else(|| infer_market(&row.get::<_, String>(0).unwrap_or_default()))
        .context("未知市场")?;
    Ok(Stock {
        code: row.get(0)?,
        name: row.get(1)?,
        market,
        added_at: row.get(3)?,
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
