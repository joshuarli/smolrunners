use rusqlite::{Connection, Params, Transaction, TransactionBehavior, types::FromSql};

use crate::store::error::{Result, StoreError, map_error};

pub const DEFAULT_READ_ROW_LIMIT: u64 = 10_000;

pub struct ReadTxn<'conn> {
    transaction: Transaction<'conn>,
    budget: u64,
}

impl<'conn> ReadTxn<'conn> {
    pub fn begin(connection: &'conn mut Connection, budget: u64) -> Result<Self> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| map_error("begin read transaction", error))?;
        Ok(Self { transaction, budget })
    }

    pub fn query<T, P>(&mut self, sql: &str, params: P) -> Result<Vec<T>>
    where
        T: FromSql,
        P: Params,
    {
        let mut statement = self
            .transaction
            .prepare(sql)
            .map_err(|error| map_error("prepare read", error))?;
        let rows = statement
            .query_map(params, |row| row.get::<_, T>(0))
            .map_err(|error| map_error("query read", error))?;
        let mut values = Vec::new();
        for row in rows {
            if values.len() as u64 >= self.budget {
                return Err(StoreError::ReadBudgetExceeded { limit: self.budget });
            }
            values.push(row.map_err(|error| map_error("read row", error))?);
        }
        Ok(values)
    }

    pub fn commit(self) -> Result<()> {
        self.transaction
            .commit()
            .map_err(|error| map_error("commit read transaction", error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection::Store;
    use crate::store::test_support::{remove_db, temp_db_path};

    #[test]
    fn store_read_transaction_enforces_its_row_budget() {
        let path = temp_db_path("read_budget");
        let mut store = Store::open(&path).expect("open");
        store
            .connection()
            .execute_batch(
                "CREATE TABLE bounded (value INTEGER NOT NULL);
                 INSERT INTO bounded VALUES (1), (2), (3);",
            )
            .expect("seed rows");
        let mut transaction =
            ReadTxn::begin(store.connection_mut(), 2).expect("begin read transaction");
        let error = transaction
            .query::<i64, _>("SELECT value FROM bounded", [])
            .expect_err("budget exceeded");
        assert_eq!(error, StoreError::ReadBudgetExceeded { limit: 2 });
        drop(transaction);
        store
            .connection()
            .execute("INSERT INTO bounded VALUES (4)", [])
            .expect("write after the guard released its transaction");
        remove_db(&path);
    }

    #[test]
    fn store_read_transaction_within_budget_commits_and_releases_locks() {
        let path = temp_db_path("read_commit");
        let mut store = Store::open(&path).expect("open");
        store
            .connection()
            .execute_batch(
                "CREATE TABLE bounded (value INTEGER NOT NULL);
                 INSERT INTO bounded VALUES (10), (20);",
            )
            .expect("seed rows");
        let mut transaction =
            ReadTxn::begin(store.connection_mut(), 2).expect("begin read transaction");
        assert_eq!(
            transaction
                .query::<i64, _>("SELECT value FROM bounded", [])
                .expect("within budget"),
            vec![10, 20]
        );
        transaction.commit().expect("commit read transaction");
        store
            .connection()
            .execute("INSERT INTO bounded VALUES (30)", [])
            .expect("write after commit");
        remove_db(&path);
    }
}
