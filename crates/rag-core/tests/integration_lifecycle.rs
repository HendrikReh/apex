//! Integration tests exercising the full document lifecycle against real
//! Postgres + Qdrant instances (started via `just up`).
//!
//! These tests require `DATABASE_URL` to be set and both services running.

use qdrant_client::Payload;
use qdrant_client::qdrant::{
    DenseVector, Distance, NamedVectors, PointStruct, Vector as QdrantVector, Vectors,
};
use rag_core::{AppConfig, Stores};
use rag_core::stores::vectors::DENSE_VECTOR_NAME;
use sqlx::Error as SqlxError;
use uuid::Uuid;

/// Helper: build a `Stores` handle from the environment.
///
/// Panics if Postgres or Qdrant are unreachable.
#[allow(clippy::disallowed_methods)] // .expect() is acceptable in test code
async fn setup_stores() -> Stores {
    let config = AppConfig::from_env().expect("AppConfig::from_env should succeed");
    Stores::new(&config).await.expect("Stores::new should succeed")
}

// ---------------------------------------------------------------------------
// Test 1 -- Full Postgres document lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() / .unwrap() are acceptable in test code
async fn full_document_lifecycle() {
    let stores = setup_stores().await;
    let collection = format!("test_lifecycle_{}", Uuid::new_v4());
    let tenant = "test-tenant";
    let doc_id = "doc-1";

    // 1. Ensure Qdrant collection (needed because some doc fields reference it).
    stores
        .ensure_collection(&collection, 4, Distance::Cosine)
        .await
        .expect("ensure_collection should succeed");

    // 2. Upsert a document (token_count left as None — update_corpus_stats sets it).
    stores
        .upsert_document(
            tenant,
            doc_id,
            "Test Doc",
            Some("en"),
            None,           // metadata
            None,           // source_path
            Some("v1"),     // version
            Some("abc123"), // checksum
            None,           // ingest_run_id
            None,           // token_count
            Some(&collection),
        )
        .await
        .expect("upsert_document should succeed");

    // 3. Insert chunks.
    let chunks = vec!["chunk 0".to_string(), "chunk 1".to_string()];
    stores.insert_chunks(tenant, doc_id, &chunks).await.expect("insert_chunks should succeed");

    // 4. Update corpus stats.
    stores
        .update_corpus_stats(tenant, &collection, doc_id, 100)
        .await
        .expect("update_corpus_stats should succeed");

    // 5. Get document -- verify fields.
    let doc = stores
        .get_document(tenant, doc_id)
        .await
        .expect("get_document should not error")
        .expect("document should exist");

    assert_eq!(doc.tenant, tenant);
    assert_eq!(doc.id, doc_id);
    assert_eq!(doc.title, "Test Doc");
    assert_eq!(doc.language.as_deref(), Some("en"));
    assert_eq!(doc.version.as_deref(), Some("v1"));
    assert_eq!(doc.checksum.as_deref(), Some("abc123"));
    assert_eq!(doc.token_count, Some(100)); // set by update_corpus_stats
    assert_eq!(doc.collection.as_deref(), Some(collection.as_str()));

    // 6. Get chunks -- verify count and order.
    let fetched_chunks = stores
        .get_chunks_by_document(tenant, doc_id)
        .await
        .expect("get_chunks_by_document should not error");

    assert_eq!(fetched_chunks.len(), 2);
    assert_eq!(fetched_chunks[0].chunk_index, 0);
    assert_eq!(fetched_chunks[0].text, "chunk 0");
    assert_eq!(fetched_chunks[1].chunk_index, 1);
    assert_eq!(fetched_chunks[1].text, "chunk 1");

    // 7. Get avgdl -- should return the stored value (100 tokens / 1 doc = 100.0).
    let avgdl =
        stores.get_avgdl(tenant, &collection, 300.0).await.expect("get_avgdl should not error");

    assert!((avgdl - 100.0).abs() < f64::EPSILON, "expected avgdl 100.0, got {avgdl}");

    // 8. Tenant isolation: different tenant sees nothing.
    let other = stores
        .get_document("other-tenant", doc_id)
        .await
        .expect("get_document for other tenant should not error");
    assert!(other.is_none(), "other tenant should not see the document");

    // 9. Delete document -- cascades to chunks.
    let deleted =
        stores.delete_document(tenant, doc_id).await.expect("delete_document should succeed");
    assert!(deleted, "delete should report a row was removed");

    // 10. Chunks should be gone (CASCADE).
    let remaining_chunks = stores
        .get_chunks_by_document(tenant, doc_id)
        .await
        .expect("get_chunks_by_document after delete should not error");
    assert!(remaining_chunks.is_empty(), "chunks should be cascade-deleted");

    // 11. Document should be gone.
    let gone = stores
        .get_document(tenant, doc_id)
        .await
        .expect("get_document after delete should not error");
    assert!(gone.is_none(), "document should be deleted");

    // 12. Cleanup: remove the Qdrant collection.
    stores.delete_collection(&collection).await.expect("delete_collection cleanup should succeed");
}

// ---------------------------------------------------------------------------
// Test 2 -- Qdrant collection and point lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() / .unwrap() are acceptable in test code
async fn qdrant_collection_lifecycle() {
    let stores = setup_stores().await;
    let collection = format!("test_qdrant_{}", Uuid::new_v4());
    let tenant = "test-tenant";
    let doc_id = "doc-1";

    // 1. Create collection.
    stores
        .ensure_collection(&collection, 4, Distance::Cosine)
        .await
        .expect("ensure_collection should succeed");

    // 2. collection_exists -> true.
    let exists =
        stores.collection_exists(&collection).await.expect("collection_exists should not error");
    assert!(exists, "collection should exist after creation");

    // 3. list_collections -> contains our collection.
    let collections = stores.list_collections().await.expect("list_collections should not error");
    assert!(collections.contains(&collection), "list_collections should include '{collection}'");

    // 4. Upsert a test point.
    let point_id = Uuid::new_v4().to_string();
    let payload = Payload::try_from(serde_json::json!({
        "tenant": tenant,
        "document_id": doc_id,
        "chunk_index": 0,
        "text": "test chunk"
    }))
    .expect("Payload construction should succeed");

    let mut named = std::collections::HashMap::new();
    named.insert(
        DENSE_VECTOR_NAME.to_string(),
        QdrantVector::from(DenseVector { data: vec![0.1_f32, 0.2, 0.3, 0.4] }),
    );
    let point = PointStruct {
        id: Some(point_id.into()),
        payload: payload.into(),
        vectors: Some(Vectors::from(NamedVectors { vectors: named })),
    };
    stores.upsert_points(&collection, vec![point]).await.expect("upsert_points should succeed");

    // 5. search_dense -> returns our point.
    let results = stores
        .search_dense(&collection, vec![0.1_f32, 0.2, 0.3, 0.4], tenant, 10)
        .await
        .expect("search_dense should not error");
    assert_eq!(results.len(), 1, "search should return exactly 1 point");

    // 6. delete_document_points -> removes the point.
    stores
        .delete_document_points(&collection, doc_id, tenant)
        .await
        .expect("delete_document_points should succeed");

    // 7. search_dense -> empty.
    let results = stores
        .search_dense(&collection, vec![0.1_f32, 0.2, 0.3, 0.4], tenant, 10)
        .await
        .expect("search_dense after delete should not error");
    assert!(results.is_empty(), "search should return no points after deletion");

    // 8. Cleanup: delete collection.
    stores.delete_collection(&collection).await.expect("delete_collection cleanup should succeed");
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() / .unwrap() are acceptable in test code
async fn anonymous_user_is_unique_per_tenant() {
    let stores = setup_stores().await;
    let tenant = format!("test-tenant-{}", Uuid::new_v4());

    sqlx::query("INSERT INTO users (id, tenant) VALUES ($1, $2)")
        .bind(Uuid::new_v4())
        .bind(&tenant)
        .execute(stores.pg_pool())
        .await
        .expect("first anonymous user insert should succeed");

    let err = sqlx::query("INSERT INTO users (id, tenant) VALUES ($1, $2)")
        .bind(Uuid::new_v4())
        .bind(&tenant)
        .execute(stores.pg_pool())
        .await
        .expect_err("second anonymous user insert should violate the partial unique index");

    match err {
        SqlxError::Database(db_err) => {
            assert_eq!(db_err.constraint(), Some("idx_users_anonymous_identity"));
        }
        other => panic!("expected database constraint error, got {other}"),
    }
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() / .unwrap() are acceptable in test code
async fn insert_chunks_requires_existing_document_for_empty_batches() {
    let stores = setup_stores().await;
    let tenant = format!("test-tenant-{}", Uuid::new_v4());
    let doc_id = "missing-doc";
    let chunks: Vec<String> = Vec::new();

    let err = stores
        .insert_chunks(&tenant, doc_id, &chunks)
        .await
        .expect_err("insert_chunks should fail when the target document does not exist");

    assert!(
        err.to_string().contains("does not exist"),
        "expected missing-document error, got {err}"
    );
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() / .unwrap() are acceptable in test code
async fn oidc_user_identity_requires_complete_unique_pair() {
    let stores = setup_stores().await;
    let tenant = format!("test-tenant-{}", Uuid::new_v4());
    let issuer = "https://issuer.example";
    let subject = "subject-123";

    sqlx::query(
        "INSERT INTO users (id, tenant, oidc_issuer, oidc_subject) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(&tenant)
    .bind(issuer)
    .bind(subject)
    .execute(stores.pg_pool())
    .await
    .expect("first OIDC user insert should succeed");

    let duplicate_err = sqlx::query(
        "INSERT INTO users (id, tenant, oidc_issuer, oidc_subject) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(&tenant)
    .bind(issuer)
    .bind(subject)
    .execute(stores.pg_pool())
    .await
    .expect_err("duplicate OIDC identity should violate the partial unique index");

    match duplicate_err {
        SqlxError::Database(db_err) => {
            assert_eq!(db_err.constraint(), Some("idx_users_oidc_identity"));
        }
        other => panic!("expected database constraint error, got {other}"),
    }

    let incomplete_err = sqlx::query(
        "INSERT INTO users (id, tenant, oidc_issuer, oidc_subject) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(&tenant)
    .bind(Option::<String>::None)
    .bind(subject)
    .execute(stores.pg_pool())
    .await
    .expect_err("incomplete OIDC identity should violate the pair check constraint");

    match incomplete_err {
        SqlxError::Database(db_err) => {
            assert_eq!(db_err.constraint(), Some("chk_users_oidc_identity_pair"));
        }
        other => panic!("expected database constraint error, got {other}"),
    }
}
