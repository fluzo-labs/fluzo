//! OBS-01 required SQLite audit path over the DAT-01 store.
//!
//! Audit records are the durable execution evidence required by A03/A09:
//! permission decisions, intent commits, outcomes and configuration
//! changes. They commit inside the same `BEGIN IMMEDIATE` unit as the
//! state updates and journal events they authorize. If a mandatory
//! audit record cannot be committed, this returns an error and the
//! dependent effect must not proceed — diagnostics can never replace
//! the journal, and losing diagnostics never relaxes `synchronous=FULL`.

use fluzo_core::telemetry::{AuditCategory, AuditRecord};

use crate::store::{EventRecord, StateUpdate, Store, StoreError};
use sqlx::Connection;
use sqlx::Row;

fn category_str(category: &AuditCategory) -> &'static str {
    match category {
        AuditCategory::PermissionDecision => "permission_decision",
        AuditCategory::IntentCommit => "intent_commit",
        AuditCategory::OutcomeCommit => "outcome_commit",
        AuditCategory::ConfigurationChange => "configuration_change",
    }
}

fn category_from(value: &str) -> AuditCategory {
    match value {
        "intent_commit" => AuditCategory::IntentCommit,
        "outcome_commit" => AuditCategory::OutcomeCommit,
        "configuration_change" => AuditCategory::ConfigurationChange,
        _ => AuditCategory::PermissionDecision,
    }
}

pub struct AuditTrail<'a> {
    store: &'a mut Store,
}

impl<'a> AuditTrail<'a> {
    pub fn new(store: &'a mut Store) -> Self {
        Self { store }
    }

    /// Commits audit records as their own durable unit.
    pub async fn commit(&mut self, records: &[AuditRecord]) -> Result<(), StoreError> {
        self.commit_with(records, &[], &[]).await
    }

    /// Commits audit records together with dependent state updates and
    /// journal events in one `BEGIN IMMEDIATE` unit. Any failure —
    /// constraint, busy, I/O — discards the whole unit and returns the
    /// error so the caller must not perform the authorized effect.
    pub async fn commit_with(
        &mut self,
        records: &[AuditRecord],
        updates: &[StateUpdate],
        events: &[EventRecord],
    ) -> Result<(), StoreError> {
        self.store.verify_drive()?;
        let mut transaction = self
            .store
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::store::map_error)?;
        for record in records {
            sqlx::query(
                "INSERT INTO audit (id, category, subject, action, outcome, correlation_id, detail, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(&record.id)
            .bind(category_str(&record.category))
            .bind(&record.subject)
            .bind(&record.action)
            .bind(&record.outcome)
            .bind(&record.correlation_id)
            .bind(&record.detail)
            .bind(record.created_unix_ms)
            .execute(&mut *transaction)
            .await
            .map_err(crate::store::map_error)?;
        }
        for update in updates {
            sqlx::query(
                "INSERT INTO state (id, kind, payload, revision) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(id) DO UPDATE SET kind = excluded.kind,
                     payload = excluded.payload, revision = state.revision + 1",
            )
            .bind(&update.id)
            .bind(&update.kind)
            .bind(&update.payload)
            .execute(&mut *transaction)
            .await
            .map_err(crate::store::map_error)?;
        }
        for event in events {
            sqlx::query("INSERT INTO event (state_id, kind, payload) VALUES (?1, ?2, ?3)")
                .bind(&event.state_id)
                .bind(&event.kind)
                .bind(&event.payload)
                .execute(&mut *transaction)
                .await
                .map_err(crate::store::map_error)?;
        }
        transaction.commit().await.map_err(crate::store::map_error)
    }

    /// All audit records in insertion order.
    pub async fn audit_records(&mut self) -> Result<Vec<AuditRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, category, subject, action, outcome, correlation_id, detail, created_ms
             FROM audit ORDER BY rowid",
        )
        .fetch_all(&mut self.store.connection)
        .await
        .map_err(crate::store::map_error)?;
        Ok(rows
            .into_iter()
            .map(|row| AuditRecord {
                id: row.try_get(0).unwrap_or_default(),
                category: category_from(&row.try_get::<String, _>(1).unwrap_or_default()),
                subject: row.try_get(2).unwrap_or_default(),
                action: row.try_get(3).unwrap_or_default(),
                outcome: row.try_get(4).unwrap_or_default(),
                correlation_id: row.try_get(5).unwrap_or_default(),
                detail: row.try_get(6).unwrap_or_default(),
                created_unix_ms: row.try_get(7).unwrap_or_default(),
            })
            .collect())
    }

    pub async fn audit_count(&mut self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COUNT(*) FROM audit")
            .fetch_one(&mut self.store.connection)
            .await
            .map_err(crate::store::map_error)?;
        row.try_get(0).map_err(crate::store::map_error)
    }
}
