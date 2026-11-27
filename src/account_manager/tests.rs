use std::path::Path;

use super::*;
use crate::render::{Freshness, TileView};

fn settings(home: &Path, interval: u64) -> TileSettings {
    TileSettings {
        codex_home: home.to_str().unwrap().to_owned(),
        codex_executable: crate::app_server::FAKE_CODEX.into(),
        refresh_minutes: interval,
        ..TileSettings::default()
    }
}

fn request_count(home: &Path) -> usize {
    std::fs::read_to_string(home.join("requests"))
        .unwrap()
        .lines()
        .count()
}

async fn tile(manager: &AccountManager, id: &str) -> TilePresentation {
    manager.state.lock().await.presentation(id).unwrap()
}

async fn force_refresh(manager: &Arc<AccountManager>, id: &str) -> RefreshResult {
    manager
        .start_manual_refresh(id.into())
        .await
        .await
        .expect("tile still has the same account")
}

#[tokio::test]
async fn switching_accounts_suppresses_pending_manual_feedback() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    manager
        .subscribe("key".into(), &settings(first.path(), 5))
        .await;
    std::fs::write(
        first.path().join("rpc-error.json"),
        r#"{"message":"sign in"}"#,
    )
    .unwrap();
    let first_feedback = manager.start_manual_refresh("key".into()).await;
    manager
        .subscribe("key".into(), &settings(second.path(), 5))
        .await;
    let second_feedback = manager.start_manual_refresh("key".into()).await;
    assert_eq!(first_feedback.await, None);
    assert_eq!(second_feedback.await, Some(Ok(())));
    assert_eq!(request_count(first.path()), 1);
    assert_eq!(request_count(second.path()), 1);
}

#[tokio::test]
async fn visible_tiles_share_refreshes_and_repainting_uses_cached_data() {
    let home = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    for id in ["a", "b"] {
        manager
            .subscribe(id.into(), &settings(home.path(), 5))
            .await;
    }
    force_refresh(&manager, "a").await.unwrap();
    manager.repaint_visible().await;
    manager.poll_due().await;
    assert_eq!(request_count(home.path()), 1);

    let (a, b) = tokio::join!(force_refresh(&manager, "a"), force_refresh(&manager, "b"));
    a.unwrap();
    b.unwrap();
    assert_eq!(request_count(home.path()), 2);
    for id in ["a", "b"] {
        assert!(matches!(
            tile(&manager, id).await.view,
            TileView::Limits {
                freshness: Freshness::Ready,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn polling_follows_the_shortest_visible_interval_and_skips_active_requests() {
    let home = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    let slow = settings(home.path(), 5);
    let account = AppServerClient::from_settings(&slow).unwrap();
    manager.subscribe("slow".into(), &slow).await;
    manager
        .subscribe("fast".into(), &settings(home.path(), 1))
        .await;
    let later = Instant::now() + Duration::from_secs(120);
    assert!(manager.state.lock().await.due_accounts(later).is_empty());
    force_refresh(&manager, "fast").await.unwrap();
    assert_eq!(
        manager.state.lock().await.due_accounts(later),
        HashSet::from([account])
    );
    manager.unsubscribe("fast").await;
    assert!(manager.state.lock().await.due_accounts(later).is_empty());
    let much_later = later + Duration::from_secs(300);
    assert_eq!(manager.state.lock().await.due_accounts(much_later).len(), 1);
    manager.unsubscribe("slow").await;
    assert!(
        manager
            .state
            .lock()
            .await
            .due_accounts(much_later)
            .is_empty()
    );
}

#[tokio::test]
async fn cancelling_a_manual_refresh_still_updates_the_key() {
    let home = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    manager
        .subscribe("key".into(), &settings(home.path(), 5))
        .await;
    force_refresh(&manager, "key").await.unwrap();

    std::fs::write(
        home.path().join("rpc-error.json"),
        r#"{"message":"sign in"}"#,
    )
    .unwrap();
    tokio::select! {
        biased;
        result = force_refresh(&manager, "key") => panic!("refresh returned early: {result:?}"),
        () = std::future::ready(()) => {},
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(
            tile(&manager, "key").await.view,
            TileView::Limits {
                freshness: Freshness::Stale,
                ..
            }
        ) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the cancelled caller's refresh should still publish its result");
    assert_eq!(request_count(home.path()), 2);
}

#[tokio::test]
async fn failures_keep_the_reading_and_settings_changes_select_the_new_account() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    manager
        .subscribe("tile".into(), &settings(first.path(), 1))
        .await;
    force_refresh(&manager, "tile").await.unwrap();
    let ready = tile(&manager, "tile").await;
    let ready_status = serde_json::to_value(&ready.status).unwrap();
    assert_eq!(ready_status["state"], "ready");
    assert!(ready_status["refreshedAt"].is_number());

    std::fs::write(
        first.path().join("rpc-error.json"),
        r#"{"message":"sign in"}"#,
    )
    .unwrap();
    assert_eq!(
        force_refresh(&manager, "tile").await,
        Err(AppServerError::NotAuthenticated)
    );
    let stale = tile(&manager, "tile").await;
    let stale_status = serde_json::to_value(&stale.status).unwrap();
    assert_eq!(stale_status["state"], "stale");
    assert_eq!(stale_status["refreshedAt"], ready_status["refreshedAt"]);
    assert!(
        matches!(stale.view, TileView::Limits { freshness: Freshness::Stale, snapshot, .. } if snapshot.windows[0].remaining_percent == 88)
    );

    manager
        .subscribe("tile".into(), &settings(second.path(), 1))
        .await;
    force_refresh(&manager, "tile").await.unwrap();
    assert_eq!(request_count(first.path()), 2);
    assert_eq!(request_count(second.path()), 1);
    assert_eq!(
        serde_json::to_value(tile(&manager, "tile").await.status).unwrap()["state"],
        "ready"
    );
}

#[tokio::test]
async fn incomplete_and_invalid_settings_have_distinct_key_states() {
    let manager = Arc::new(AccountManager::default());
    manager
        .subscribe("key".into(), &TileSettings::default())
        .await;
    assert!(matches!(
        tile(&manager, "key").await.view,
        TileView::Unconfigured
    ));
    assert!(matches!(
        force_refresh(&manager, "key").await,
        Err(AppServerError::HomeNotSelected)
    ));

    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    manager
        .subscribe("key".into(), &settings(&missing, 5))
        .await;
    assert!(
        matches!(tile(&manager, "key").await.view, TileView::Error { message, .. } if message == "Bad home")
    );
}

#[tokio::test]
#[ignore = "manual release-mode performance measurement"]
async fn benchmark_polling() {
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(AccountManager::default());
    for index in 0..32 {
        let home = root.path().join((index % 4).to_string());
        std::fs::create_dir_all(&home).unwrap();
        manager
            .subscribe(index.to_string(), &settings(&home, 5))
            .await;
    }
    for index in 0..4 {
        force_refresh(&manager, &index.to_string()).await.unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..10_000 {
            manager.poll_due().await;
        }
        samples.push(start.elapsed().as_nanos() / 10_000);
    }
    samples.sort_unstable();
    println!(
        "poll 32 tiles / 4 accounts: {} ns/pass (median of 7 samples)",
        samples[3]
    );
}
