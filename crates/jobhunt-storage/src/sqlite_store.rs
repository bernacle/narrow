//! [`Store`] and [`SyncLedger`] for the local SQLite store.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jobhunt_core::SourceKey;
use jobhunt_jobs::verification::VerificationRecord;
use jobhunt_jobs::{JobId, OpportunityId, StorageError};
use jobhunt_profile::entities::{EntityKey, EntityKind};
use jobhunt_ranking::FeedbackId;
use sqlx::Row;

use crate::sqlite::{SqliteJobStore, StoreStats, decode_timestamp, encode_timestamp};
use crate::state::{StateImport, StateImported};
use crate::store::{ScanRecord, Store, WriteGuard};
use crate::sync::{EntityBase, LedgerUpdate, SyncAccount, SyncConflict, SyncLedger};

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> StorageError {
    move |source| StorageError::Query {
        operation,
        source: Box::new(source),
    }
}

fn corrupt(id: &str, detail: impl ToString) -> StorageError {
    StorageError::Corrupt {
        id: id.to_owned(),
        detail: detail.to_string(),
    }
}

#[async_trait]
impl Store for SqliteJobStore {
    fn location(&self) -> String {
        SqliteJobStore::location(self).to_owned()
    }

    async fn opportunities_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<OpportunityId>, StorageError> {
        SqliteJobStore::opportunities_with_prefix(self, prefix, limit).await
    }

    async fn jobs_with_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<JobId>, StorageError> {
        SqliteJobStore::jobs_with_prefix(self, prefix, limit).await
    }

    async fn last_checked(&self) -> Result<HashMap<SourceKey, DateTime<Utc>>, StorageError> {
        SqliteJobStore::last_checked(self).await
    }

    async fn recent_scans(
        &self,
        per_source: usize,
    ) -> Result<HashMap<SourceKey, Vec<ScanRecord>>, StorageError> {
        SqliteJobStore::recent_scans(self, per_source).await
    }

    async fn stats(&self) -> Result<StoreStats, StorageError> {
        SqliteJobStore::stats(self).await
    }

    async fn import_state(
        &self,
        state: StateImport<'_>,
    ) -> Result<StateImported, jobhunt_profile::StorageError> {
        SqliteJobStore::import_state(self, state).await
    }

    async fn import_verification(&self, record: &VerificationRecord) -> Result<bool, StorageError> {
        self.insert_verification(record, true).await
    }

    async fn write_lock(&self) -> Result<WriteGuard, StorageError> {
        Ok(WriteGuard::new(self.writes.clone().lock_owned().await))
    }

    fn sync_ledger(&self) -> Option<&dyn SyncLedger> {
        Some(self)
    }

    async fn shutdown(&self) {
        self.pool.close().await;
    }
}

fn json_text(value: Option<&serde_json::Value>) -> Option<String> {
    value.map(serde_json::Value::to_string)
}

fn parse_json(id: &str, text: Option<String>) -> Result<Option<serde_json::Value>, StorageError> {
    text.map(|t| serde_json::from_str(&t).map_err(|e| corrupt(id, e)))
        .transpose()
}

fn parse_kind(id: &str, kind: &str) -> Result<EntityKind, StorageError> {
    kind.parse().map_err(|e: String| corrupt(id, e))
}

#[async_trait]
impl SyncLedger for SqliteJobStore {
    async fn sync_account(&self) -> Result<Option<SyncAccount>, StorageError> {
        let Some(row) =
            sqlx::query("SELECT server, user_id, cursor, last_sync_at FROM sync_account")
                .fetch_optional(&self.pool)
                .await
                .map_err(query_error("loading the sync account"))?
        else {
            return Ok(None);
        };
        let get = |c: &str| -> Result<String, StorageError> {
            row.try_get(c).map_err(|e| corrupt("sync_account", e))
        };
        let cursor: i64 = row
            .try_get("cursor")
            .map_err(|e| corrupt("sync_account", e))?;
        let last: Option<String> = row
            .try_get("last_sync_at")
            .map_err(|e| corrupt("sync_account", e))?;
        Ok(Some(SyncAccount {
            server: get("server")?,
            user_id: get("user_id")?,
            cursor: u64::try_from(cursor).unwrap_or(0),
            last_sync_at: last
                .map(|t| decode_timestamp(&t).map_err(|e| corrupt("sync_account", e)))
                .transpose()?,
        }))
    }

    async fn sync_bases(&self) -> Result<Vec<EntityBase>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, id, version, deleted, digest, body FROM sync_entities ORDER BY kind, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading sync bases"))?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("id").map_err(|e| corrupt("sync_entities", e))?;
                let bad = |e: sqlx::Error| corrupt(&id, e);
                let kind: String = row.try_get("kind").map_err(bad)?;
                let version: i64 = row.try_get("version").map_err(bad)?;
                Ok(EntityBase {
                    key: EntityKey::new(parse_kind(&id, &kind)?, id.clone()),
                    version: u64::try_from(version).unwrap_or(0),
                    deleted: row.try_get("deleted").map_err(bad)?,
                    digest: row.try_get("digest").map_err(bad)?,
                    body: parse_json(&id, row.try_get("body").map_err(bad)?)?,
                })
            })
            .collect()
    }

    async fn sync_conflicts(&self) -> Result<Vec<SyncConflict>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, id, reason, local_body, cloud_body, cloud_version, detected_at \
             FROM sync_conflicts ORDER BY kind, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(query_error("loading sync conflicts"))?;
        rows.iter()
            .map(|row| {
                let id: String = row
                    .try_get("id")
                    .map_err(|e| corrupt("sync_conflicts", e))?;
                let bad = |e: sqlx::Error| corrupt(&id, e);
                let kind: String = row.try_get("kind").map_err(bad)?;
                let version: i64 = row.try_get("cloud_version").map_err(bad)?;
                let at: String = row.try_get("detected_at").map_err(bad)?;
                Ok(SyncConflict {
                    key: EntityKey::new(parse_kind(&id, &kind)?, id.clone()),
                    reason: row.try_get("reason").map_err(bad)?,
                    local: parse_json(&id, row.try_get("local_body").map_err(bad)?)?,
                    cloud: parse_json(&id, row.try_get("cloud_body").map_err(bad)?)?,
                    cloud_version: u64::try_from(version).unwrap_or(0),
                    detected_at: decode_timestamp(&at).map_err(|e| corrupt(&id, e))?,
                })
            })
            .collect()
    }

    async fn synced_feedback(&self) -> Result<Vec<FeedbackId>, StorageError> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM sync_feedback ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(query_error("loading synced feedback"))?;
        ids.iter()
            .map(|id| id.parse().map_err(|e| corrupt(id, e)))
            .collect()
    }

    async fn update_ledger(&self, update: &LedgerUpdate) -> Result<(), StorageError> {
        let mut tx = self
            .begin_write()
            .await
            .map_err(query_error("starting a transaction"))?;
        if let Some(a) = &update.account {
            sqlx::query(
                "INSERT INTO sync_account (singleton, server, user_id, cursor, last_sync_at) \
                 VALUES (1, ?, ?, ?, ?) ON CONFLICT (singleton) DO UPDATE SET \
                 server = excluded.server, user_id = excluded.user_id, \
                 cursor = excluded.cursor, last_sync_at = excluded.last_sync_at",
            )
            .bind(&a.server)
            .bind(&a.user_id)
            .bind(i64::try_from(a.cursor).unwrap_or(i64::MAX))
            .bind(a.last_sync_at.map(encode_timestamp))
            .execute(&mut *tx)
            .await
            .map_err(query_error("saving the sync account"))?;
        }
        for b in &update.bases {
            sqlx::query(
                "INSERT INTO sync_entities (kind, id, version, deleted, digest, body) \
                 VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (kind, id) DO UPDATE SET \
                 version = excluded.version, deleted = excluded.deleted, \
                 digest = excluded.digest, body = excluded.body",
            )
            .bind(b.key.kind.as_str())
            .bind(&b.key.id)
            .bind(i64::try_from(b.version).unwrap_or(i64::MAX))
            .bind(b.deleted)
            .bind(&b.digest)
            .bind(json_text(b.body.as_ref()))
            .execute(&mut *tx)
            .await
            .map_err(query_error("saving a sync base"))?;
        }
        for key in &update.resolved {
            sqlx::query("DELETE FROM sync_conflicts WHERE kind = ? AND id = ?")
                .bind(key.kind.as_str())
                .bind(&key.id)
                .execute(&mut *tx)
                .await
                .map_err(query_error("resolving a sync conflict"))?;
        }
        for c in &update.conflicts {
            sqlx::query(
                "INSERT INTO sync_conflicts (kind, id, reason, local_body, cloud_body, \
                 cloud_version, detected_at) VALUES (?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (kind, id) DO UPDATE SET reason = excluded.reason, \
                 local_body = excluded.local_body, cloud_body = excluded.cloud_body, \
                 cloud_version = excluded.cloud_version, detected_at = excluded.detected_at",
            )
            .bind(c.key.kind.as_str())
            .bind(&c.key.id)
            .bind(&c.reason)
            .bind(json_text(c.local.as_ref()))
            .bind(json_text(c.cloud.as_ref()))
            .bind(i64::try_from(c.cloud_version).unwrap_or(i64::MAX))
            .bind(encode_timestamp(c.detected_at))
            .execute(&mut *tx)
            .await
            .map_err(query_error("saving a sync conflict"))?;
        }
        for id in &update.feedback_synced {
            sqlx::query("INSERT OR IGNORE INTO sync_feedback (id) VALUES (?)")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(query_error("marking feedback as synced"))?;
        }
        tx.commit()
            .await
            .map_err(query_error("committing sync bookkeeping"))
    }

    async fn reset_ledger(&self) -> Result<(), StorageError> {
        let mut tx = self
            .begin_write()
            .await
            .map_err(query_error("starting a transaction"))?;
        for table in [
            "sync_account",
            "sync_entities",
            "sync_conflicts",
            "sync_feedback",
        ] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&mut *tx)
                .await
                .map_err(query_error("resetting sync state"))?;
        }
        tx.commit()
            .await
            .map_err(query_error("committing a sync reset"))
    }
}
