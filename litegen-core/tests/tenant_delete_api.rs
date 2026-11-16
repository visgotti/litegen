//! Deleting an org deletes its apps' files in litegen's own store
//! (docs/storage-access.md, "Deleting an org or an app"). The app route's
//! twin is `deleting_an_app_deletes_its_files_in_litegens_store`
//! (retention_api.rs).

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use tower::ServiceExt;

fn files_under(dir: &std::path::Path, id: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.to_string_lossy().contains(id) {
                found.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn deleting_an_org_deletes_its_apps_files_in_litegens_store() {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let now = chrono::Utc::now();
    let org = litegen::types::Organization {
        id: "org-going".into(),
        name: "Going".into(),
        slug: "going".into(),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let mut ids = Vec::new();
    for n in 0..2 {
        let application = litegen::types::Application {
            id: format!("app-going-{n}"),
            org_id: org.id.clone(),
            name: format!("Going {n}"),
            slug: format!("going-{n}"),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        db.create_application(&application).await.unwrap();
        let id = format!("litegen-3d-org-going-{n}");
        let url = state
            .router
            .model3d_store
            .put(&format!("litegen/3d/{id}/model.glb"), &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary")
            .await
            .unwrap();
        db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(&org.id), Some(&application.id)).await.unwrap();
        let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
        db.update_generation_if_active(&id, "completed", 100, Some(&url), None, Some(now), Some(&meta)).await.unwrap();
        assert_eq!(files_under(&dir, &id).len(), 1);
        ids.push(id);
    }

    let resp = app.clone().oneshot(Request::delete(format!("/v1/orgs/{}", org.id)).body(Body::empty()).unwrap()).await.unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    // Deleted on a task of their own, right after the delete.
    for _ in 0..1500 {
        if ids.iter().all(|id| files_under(&dir, id).is_empty()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for id in &ids {
        assert!(files_under(&dir, id).is_empty(), "{id}: the org's files in litegen's store went with it");
        assert!(db.get_generation(id).await.unwrap().is_none(), "{id}: and its row");
    }
}
