//! Read-only, bounded activity from durable newest-request/run metadata.

use std::collections::{BTreeMap, BTreeSet};

use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;
use uuid::Uuid;

use crate::persistence::{
    AiMessageRecord, AiRunFailureDispositionRecord, AiRunRecord, AiSessionEventRecord,
    AiSessionRecord,
};
use crate::{AiError, AiSessionActivity, AiSessionActivityState, AiSessionEdge};

const SESSIONS: &str = "graphql_orm_ai_sessions";
const EVENTS: &str = "graphql_orm_ai_session_events";
const MESSAGES: &str = "graphql_orm_ai_messages";
const RUNS: &str = "graphql_orm_ai_runs";
const DISPOSITIONS: &str = "graphql_orm_ai_run_failure_dispositions";
const REQUESTS: &str = "session_activity_requests";
const MAX_SESSIONS: usize = 200;

fn safe<T, E>(result: Result<T, E>) -> Result<T, AiError> {
    result.map_err(|_| AiError::PersistenceFailed)
}

fn field(
    schema: &ValidatedRuntimeSchema,
    table: &str,
    name: &str,
) -> Result<RuntimeFieldHandle, AiError> {
    let collection = safe(schema.resolve_collection(&safe(CollectionId::new(table))?))?;
    safe(schema.resolve_field(&collection, &safe(FieldId::new(format!("{table}.{name}")))?))
}

fn uuid(
    schema: &ValidatedRuntimeSchema,
    row: &RuntimeRecord,
    table: &str,
    name: &str,
) -> Result<Uuid, AiError> {
    safe(row.uuid(&field(schema, table, name)?))
}

fn text<'a>(
    schema: &ValidatedRuntimeSchema,
    row: &'a RuntimeRecord,
    table: &str,
    name: &str,
) -> Result<&'a str, AiError> {
    safe(row.string(&field(schema, table, name)?))
}

fn limits() -> RuntimeQueryLimits {
    RuntimeQueryLimits {
        max_page_size: MAX_SESSIONS as u32,
        max_values_per_list: MAX_SESSIONS,
        ..Default::default()
    }
}

/// Ephemeral query description derived from owning entities, never a schema
/// synchronization/migration or a second persistent projection.
fn query_schema() -> Result<ValidatedRuntimeSchema, AiError> {
    let mut schema = safe(RuntimeSchema::from_static_entities(&[
        AiSessionRecord::metadata(),
        AiSessionEventRecord::metadata(),
        AiMessageRecord::metadata(),
        AiRunRecord::metadata(),
        AiRunFailureDispositionRecord::metadata(),
    ]))?;
    // These private primary keys already define the durable keyset identity.
    // The fixed server-authored query admits them only as deterministic tie
    // breakers; this does not expose a new client-selectable sort surface.
    for collection in &mut schema.collections {
        for field in &mut collection.fields {
            if collection.primary_key.contains(&field.id) {
                field.sortable = true;
            }
        }
    }
    let collection = schema
        .collections
        .iter_mut()
        .find(|item| item.physical_table == SESSIONS)
        .ok_or(AiError::PersistenceFailed)?;
    collection.relations.push(RuntimeRelation {
        id: safe(RelationId::new(REQUESTS))?,
        api_name: REQUESTS.to_owned(),
        target: safe(CollectionId::new(EVENTS))?,
        key_pairs: vec![RelationKeyPair {
            source: safe(FieldId::new(format!("{SESSIONS}.id")))?,
            target: safe(FieldId::new(format!("{EVENTS}.session_id")))?,
        }],
        cardinality: RelationCardinality::Many,
        enforce_foreign_key: false,
        on_delete: None,
    });
    safe(schema.validate())
}

pub(crate) fn activity_state(state: &str) -> Option<AiSessionActivityState> {
    use AiSessionActivityState::*;
    match state {
        "queued"
        | "leased"
        | "running"
        | "waiting_tool"
        | "waiting_provider"
        | "waiting_subscription"
        | "retry_scheduled" => Some(Working),
        "waiting_approval" | "waiting_reauth" => Some(Prompt),
        "completed" => Some(Done),
        "failed" | "cancelled" | "recovery_required" => Some(Error),
        _ => None,
    }
}

async fn read_metadata(
    database: &Database<DefaultWriteBackend>,
    schema: &ValidatedRuntimeSchema,
    table: &str,
    key: &str,
    ids: Vec<Uuid>,
    columns: &[&str],
) -> Result<BTreeMap<Uuid, RuntimeRecord>, AiError> {
    let ids = ids.into_iter().collect::<BTreeSet<_>>();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    if ids.len() > MAX_SESSIONS {
        return Err(AiError::PersistenceFailed);
    }
    let collection = safe(schema.resolve_collection(&safe(CollectionId::new(table))?))?;
    let fields = columns
        .iter()
        .map(|column| field(schema, table, column))
        .collect::<Result<Vec<_>, _>>()?;
    let key_field = field(schema, table, key)?;
    let predicate = safe(schema.runtime_list(
        &collection,
        &key_field,
        RuntimeListOperator::In,
        ids.iter().copied().map(RuntimeValue::Uuid).collect(),
        limits(),
    ))?;
    let request = safe(schema.runtime_read_request(
        &collection,
        &safe(schema.resolve_projection(&collection, &fields))?,
        Some(predicate),
        safe(schema.runtime_order(&collection, None, limits()))?,
        RuntimePageRequest::first(ids.len() as i64, None),
        false,
        limits(),
    ))?;
    let result = safe(database.execute_runtime_read(&request, None).await)?;
    if result.page_info.has_next_page {
        return Err(AiError::PersistenceFailed);
    }
    result
        .edges
        .into_iter()
        .map(|edge| Ok((safe(edge.node.uuid(&key_field))?, edge.node)))
        .collect()
}

/// At most five metadata queries for the entire page: owned anchors, one latest
/// run-request shell per session, retry dispositions if needed, runs, and inputs.
/// The ORM batches one LIMIT 2 (one result plus sentinel) branch per parent into
/// one statement. It never hydrates full history or protected payloads.
pub(crate) async fn session_activity(
    database: &Database<DefaultWriteBackend>,
    principal_kind: &str,
    principal_subject: &str,
    edges: &[AiSessionEdge],
) -> Result<Vec<AiSessionActivity>, AiError> {
    if edges.len() > MAX_SESSIONS {
        return Err(AiError::InvalidInput(
            "session activity page exceeds 200 rows".to_owned(),
        ));
    }
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let schema = query_schema()?;
    let sessions = safe(schema.resolve_collection(&safe(CollectionId::new(SESSIONS))?))?;
    let events = safe(schema.resolve_collection(&safe(CollectionId::new(EVENTS))?))?;
    let requests = safe(schema.resolve_relation(&sessions, &safe(RelationId::new(REQUESTS))?))?;
    let session_id = field(&schema, SESSIONS, "id")?;
    let message_head = field(&schema, SESSIONS, "message_head")?;
    let owned = safe(schema.runtime_and(
        &sessions,
        vec![
                safe(
                    schema.runtime_list(
                        &sessions,
                        &session_id,
                        RuntimeListOperator::In,
                        edges
                            .iter()
                            .map(|edge| RuntimeValue::Uuid(edge.node.id))
                            .collect(),
                        limits(),
                    ),
                )?,
                safe(schema.runtime_compare(
                    &sessions,
                    &field(&schema, SESSIONS, "owner_principal_kind")?,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::String(principal_kind.to_owned()),
                    limits(),
                ))?,
                safe(schema.runtime_compare(
                    &sessions,
                    &field(&schema, SESSIONS, "owner_subject")?,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::String(principal_subject.to_owned()),
                    limits(),
                ))?,
                safe(schema.runtime_list(
                    &sessions,
                    &field(&schema, SESSIONS, "state")?,
                    RuntimeListOperator::In,
                    vec![
                        RuntimeValue::String("active".to_owned()),
                        RuntimeValue::String("archived".to_owned()),
                    ],
                    limits(),
                ))?,
            ],
        limits(),
    ))?;
    let parents = safe(
        database
            .execute_runtime_anchored_read(
                &safe(schema.runtime_read_request_with_relation_keys(
                    &sessions,
                    &safe(schema.resolve_projection(
                        &sessions,
                        &[session_id.clone(), message_head.clone()],
                    ))?,
                    Some(owned),
                    safe(schema.runtime_order(&sessions, None, limits()))?,
                    RuntimePageRequest::first(edges.len() as i64, None),
                    false,
                    std::slice::from_ref(&requests),
                    limits(),
                ))?,
                None,
            )
            .await,
    )?;
    if parents.edges.len() != edges.len() {
        return Err(AiError::Conflict);
    }
    let anchors = safe(parents.relation_parents(&requests))?;
    let event_run = field(&schema, EVENTS, "run_id")?;
    let event_type = field(&schema, EVENTS, "event_type")?;
    let sequence = field(&schema, EVENTS, "sequence")?;
    let queued = safe(schema.runtime_list(
        &events,
        &event_type,
        RuntimeListOperator::In,
        vec![
            RuntimeValue::String("message_queued".to_owned()),
            RuntimeValue::String("run_retry_queued".to_owned()),
        ],
        limits(),
    ))?;
    let order = safe(schema.runtime_order(
        &events,
        Some(vec![RuntimeOrderInput {
            field: sequence,
            direction: RuntimeOrderDirection::Desc,
            nulls: RuntimeNullPlacement::Last,
        }]),
        limits(),
    ))?;
    let request_batch =
        safe(
            database
                .execute_runtime_relation_batch(
                    &safe(schema.runtime_relation_batch_request(
                        &requests,
                        anchors,
                        &safe(schema.resolve_projection(
                            &events,
                            &[event_run.clone(), event_type.clone()],
                        ))?,
                        Some(queued),
                        order,
                        RuntimeRelationSelection::ToMany {
                            pages: vec![RuntimePageRequest::first(1, None); parents.edges.len()],
                            include_count: false,
                        },
                        RuntimeRelationLimits {
                            max_parents: MAX_SESSIONS,
                            max_compatible_groups: MAX_SESSIONS,
                            max_bind_parameters: 999,
                            ..Default::default()
                        },
                    ))?,
                    None,
                )
                .await,
        )?;
    let mut chosen = BTreeMap::new();
    let mut retry_sources = Vec::new();
    let mut activities = BTreeMap::new();
    for (parent, result) in parents.edges.iter().zip(request_batch.results) {
        let id = safe(parent.node.uuid(&session_id))?;
        let RuntimeRelationValue::ToMany(events) = result.value else {
            return Err(AiError::PersistenceFailed);
        };
        let mut activity = AiSessionActivity {
            session_id: id,
            state: None,
            run_id: None,
            input_message_sequence: None,
            started_at: None,
        };
        if let Some(event) = events.edges.first() {
            let is_retry = safe(event.node.string(&event_type))? == "run_retry_queued";
            if let RuntimeFieldState::Value(RuntimeValue::Uuid(run_id)) =
                safe(event.node.state(&event_run))?
            {
                chosen.insert(id, (*run_id, is_retry));
                if is_retry {
                    retry_sources.push(*run_id);
                }
            }
        } else if safe(parent.node.integer(&message_head))? == 0 {
            activity.state = Some(AiSessionActivityState::Prompt);
        }
        activities.insert(id, activity);
    }
    let dispositions = read_metadata(
        database,
        &schema,
        DISPOSITIONS,
        "source_run_id",
        retry_sources,
        &[
            "source_run_id",
            "session_id",
            "principal_kind",
            "principal_subject",
            "input_message_id",
            "disposition",
            "retry_run_id",
        ],
    )
    .await?;
    let mut selected = BTreeMap::new();
    for (id, (source_run, is_retry)) in chosen {
        if is_retry {
            let Some(disposition) = dispositions.get(&source_run) else {
                continue;
            };
            if uuid(&schema, disposition, DISPOSITIONS, "session_id")? != id
                || text(&schema, disposition, DISPOSITIONS, "principal_kind")? != principal_kind
                || text(&schema, disposition, DISPOSITIONS, "principal_subject")?
                    != principal_subject
                || text(&schema, disposition, DISPOSITIONS, "disposition")? != "retried"
            {
                return Err(AiError::PersistenceFailed);
            }
            selected.insert(
                id,
                (
                    uuid(&schema, disposition, DISPOSITIONS, "retry_run_id")?,
                    Some(uuid(
                        &schema,
                        disposition,
                        DISPOSITIONS,
                        "input_message_id",
                    )?),
                ),
            );
        } else {
            selected.insert(id, (source_run, None));
        }
    }
    let runs = read_metadata(
        database,
        &schema,
        RUNS,
        "id",
        selected.values().map(|(id, _)| *id).collect(),
        &[
            "id",
            "session_id",
            "input_message_id",
            "state",
            "created_at",
        ],
    )
    .await?;
    let input_ids = runs
        .values()
        .map(|row| uuid(&schema, row, RUNS, "input_message_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let messages = read_metadata(
        database,
        &schema,
        MESSAGES,
        "id",
        input_ids,
        &["id", "session_id", "message_role", "sequence"],
    )
    .await?;
    for (id, (run_id, retry_input)) in selected {
        let Some(run) = runs.get(&run_id) else {
            continue;
        };
        if uuid(&schema, run, RUNS, "session_id")? != id {
            return Err(AiError::PersistenceFailed);
        }
        let input_id = uuid(&schema, run, RUNS, "input_message_id")?;
        if retry_input.is_some_and(|expected| expected != input_id) {
            return Err(AiError::PersistenceFailed);
        }
        let Some(input) = messages.get(&input_id) else {
            continue;
        };
        if uuid(&schema, input, MESSAGES, "session_id")? != id
            || text(&schema, input, MESSAGES, "message_role")? != "user"
        {
            return Err(AiError::PersistenceFailed);
        }
        let activity = activities.get_mut(&id).ok_or(AiError::PersistenceFailed)?;
        activity.run_id = Some(run_id);
        activity.input_message_sequence =
            Some(safe(input.integer(&field(&schema, MESSAGES, "sequence")?))?);
        activity.started_at = Some(safe(run.integer(&field(&schema, RUNS, "created_at")?))?);
        activity.state = activity_state(text(&schema, run, RUNS, "state")?);
    }
    edges
        .iter()
        .map(|edge| {
            activities
                .remove(&edge.node.id)
                .ok_or(AiError::PersistenceFailed)
        })
        .collect()
}
#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::persistence::*;
    use graphql_orm::graphql::errors::OrmPublicError;
    use graphql_orm::graphql::orm::OrmSchemaModule;

    async fn database() -> Database<DefaultWriteBackend> {
        let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "activity-tests",
                "Activity metadata tests",
                crate::AiSchemaModule.entities(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        db
    }

    async fn seed(
        db: &Database<DefaultWriteBackend>,
        owner: &str,
        states: &[&str],
        extra_assistant: bool,
    ) -> AiSessionEdge {
        let owner = owner.to_owned();
        let states = states.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let session = db
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let id = Uuid::new_v4();
                    let session = tx
                        .insert::<AiSessionRecord>(CreateAiSessionRecordInput {
                            execution_selection: None,
                            id,
                            owner_principal_kind: "user".to_owned(),
                            owner_subject: owner,
                            tenant_id: Some("tenant".to_owned()),
                            scope_kind: "workspace".to_owned(),
                            scope_id: "scope".to_owned(),
                            title: "Metadata only".to_owned(),
                            title_revision: 0,
                            title_source: "user".to_owned(),
                            state: "active".to_owned(),
                            stream_head: 0,
                            message_head: states.len() as i64 + i64::from(extra_assistant),
                            last_activity_at: 100,
                            archived_at: None,
                            deleted_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    for (i, state) in states.into_iter().enumerate() {
                        let message_id = Uuid::new_v4();
                        let run_id = Uuid::new_v4();
                        tx.insert::<AiMessageRecord>(CreateAiMessageRecordInput {
                            id: message_id,
                            session_id: id,
                            sequence: i as i64 + 1,
                            message_role: "user".to_owned(),
                            author_principal_kind: None,
                            author_subject: None,
                            client_message_id: None,
                            content_hash: None,
                            run_id: Some(run_id),
                            provider_kind: None,
                            provider_model: None,
                            protected_preview: None,
                            block_count: 0,
                            completion_state: "complete".to_owned(),
                            finalized_at: None,
                            content_purged_at: Some(100),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: Uuid::new_v4(),
                            session_id: id,
                            sequence: i as i64 + 1,
                            event_type: "message_queued".to_owned(),
                            run_id: Some(run_id),
                            causation_id: None,
                            correlation_id: "fixture".to_owned(),
                            protected_payload: serde_json::json!({"unreadable": true}),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiRunRecord>(CreateAiRunRecordInput {
                            execution_selection: None,
                            id: run_id,
                            session_id: id,
                            input_message_id: message_id,
                            principal_reference: serde_json::json!({}),
                            state,
                            attempt_id: None,
                            lease_owner: None,
                            lease_generation: 0,
                            lease_expires_at: None,
                            lease_heartbeat_at: None,
                            retry_count: 0,
                            next_attempt_at: None,
                            error_code: None,
                            latest_checkpoint_id: None,
                            cancellation_request_id: None,
                            cancellation_requested_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    if extra_assistant {
                        tx.insert::<AiMessageRecord>(CreateAiMessageRecordInput {
                            id: Uuid::new_v4(),
                            session_id: id,
                            sequence: session.message_head,
                            message_role: "assistant".to_owned(),
                            author_principal_kind: None,
                            author_subject: None,
                            client_message_id: None,
                            content_hash: None,
                            run_id: None,
                            provider_kind: None,
                            provider_model: None,
                            protected_preview: None,
                            block_count: 0,
                            completion_state: "complete".to_owned(),
                            finalized_at: None,
                            content_purged_at: Some(100),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    Ok(session)
                })
            })
            .await
            .unwrap();
        AiSessionEdge {
            node: crate::orm_sessions::session_view(&session).unwrap(),
            cursor: "unused".to_owned(),
        }
    }

    #[test]
    fn activity_query_schema_and_all_run_states_are_closed() {
        query_schema().unwrap();
        for state in [
            "queued",
            "leased",
            "running",
            "waiting_tool",
            "waiting_provider",
            "waiting_subscription",
            "retry_scheduled",
        ] {
            assert_eq!(activity_state(state), Some(AiSessionActivityState::Working));
        }
        for state in ["waiting_approval", "waiting_reauth"] {
            assert_eq!(activity_state(state), Some(AiSessionActivityState::Prompt));
        }
        for state in ["failed", "cancelled", "recovery_required"] {
            assert_eq!(activity_state(state), Some(AiSessionActivityState::Error));
        }
        assert_eq!(
            activity_state("completed"),
            Some(AiSessionActivityState::Done)
        );
        assert_eq!(activity_state("invented"), None);
    }

    #[tokio::test]
    async fn newest_input_wins_with_no_inbox_or_content_and_late_old_completion() {
        let db = database().await;
        let edge = seed(&db, "owner", &["completed", "queued"], true).await;
        let result = session_activity(&db, "user", "owner", &[edge])
            .await
            .unwrap();
        assert_eq!(result[0].state, Some(AiSessionActivityState::Working));
        assert_eq!(result[0].input_message_sequence, Some(2));
        let run_id = result[0].run_id.unwrap();
        // Complete a DIFFERENT older run after the newer input exists. Arrival
        // order and timestamps must never replace the durable input ordering.
        db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                let rows = tx
                    .query::<AiRunRecord>()
                    .limit(2)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)?;
                let older = rows.iter().find(|row| row.id != run_id).unwrap();
                tx.compare_and_swap::<AiRunRecord>(
                    &older.id,
                    older.row_version,
                    AiRunRecordWhereInput::default(),
                    UpdateAiRunRecordInput {
                        state: Some("failed".to_owned()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();
        let session = db
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.query::<AiSessionRecord>()
                        .limit(1)
                        .fetch_one()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .unwrap()
            .unwrap();
        let edge = AiSessionEdge {
            node: crate::orm_sessions::session_view(&session).unwrap(),
            cursor: "unused".to_owned(),
        };
        let after = session_activity(&db, "user", "owner", &[edge])
            .await
            .unwrap();
        assert_eq!(after[0].run_id, Some(run_id));
        assert_eq!(after[0].state, Some(AiSessionActivityState::Working));
    }

    #[tokio::test]
    async fn page_of_200_reads_only_newest_input_and_preserves_page_order() {
        let db = database().await;
        let states = [
            "failed",
            "completed",
            "cancelled",
            "waiting_approval",
            "waiting_reauth",
            "running",
        ];
        let mut edges = Vec::new();
        for i in 0..200 {
            edges.push(
                seed(
                    &db,
                    "owner",
                    &["completed", "failed", states[i % states.len()]],
                    true,
                )
                .await,
            );
        }
        let result = session_activity(&db, "user", "owner", &edges)
            .await
            .unwrap();
        assert_eq!(result.len(), 200);
        for (i, activity) in result.iter().enumerate() {
            assert_eq!(activity.session_id, edges[i].node.id);
            assert_eq!(activity.input_message_sequence, Some(3));
            assert_eq!(activity.state, activity_state(states[i % states.len()]));
        }
        assert!(matches!(
            session_activity(&db, "user", "stranger", &edges).await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            session_activity(&db, "api_token:service", "owner", &edges).await,
            Err(AiError::Conflict)
        ));
    }

    #[tokio::test]
    async fn empty_is_prompt_but_missing_run_evidence_remains_unknown() {
        let db = database().await;
        let empty = seed(&db, "owner", &[], false).await;
        let mut missing = seed(&db, "owner", &["completed"], false).await;
        let id = missing.node.id;
        db.transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                let rows = tx
                    .query::<AiRunRecord>()
                    .filter(AiRunRecordWhereInput {
                        session_id: Some(graphql_orm::graphql::filters::UuidFilter {
                            eq: Some(id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .limit(1)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)?;
                tx.delete_by_id::<AiRunRecord>(&rows[0].id)
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();
        missing.cursor = "missing".to_owned();
        let result = session_activity(&db, "user", "owner", &[empty, missing])
            .await
            .unwrap();
        assert_eq!(result[0].state, Some(AiSessionActivityState::Prompt));
        assert_eq!(result[1].state, None);
    }
}
