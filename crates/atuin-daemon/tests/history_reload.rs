//! `atuin import` writes straight to the history db. The daemon's search index only learns about
//! those rows when asked to reload them.
#![cfg(unix)]

mod common;

use atuin_client::history::History;
use common::{TestEnv, history};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

/// Rows saved like `atuin import` does: directly into the db, bypassing the daemon.
#[fixture]
fn imported() -> Vec<History> {
    (0..3).map(|i| history(&format!("echo imported {i}"))).collect()
}

#[rstest]
#[tokio::test]
async fn reload_indexes_rows_written_directly_to_the_db(
    #[future(awt)] env: TestEnv,
    imported: Vec<History>,
) {
    let mut client = env.history_client().await;
    let mut search = env.search_client().await;
    env.history_db.save_bulk(&imported).await.unwrap();

    assert!(env.rpc_hits(&mut search, "imported").await.is_empty(), "stale until reloaded");

    let reply = client.reload_history().await.unwrap();
    assert_eq!(reply.protocol, 6);

    let mut hits = env.rpc_hits(&mut search, "imported").await;
    let mut expected: Vec<_> = imported.iter().map(|h| h.id).collect();
    hits.sort();
    expected.sort();
    assert_eq!(hits, expected);
}

/// Reloading keeps commands the daemon recorded itself, and leaves running commands in flight.
#[rstest]
#[tokio::test]
async fn reload_keeps_daemon_recorded_and_in_flight_commands(
    #[future(awt)] env: TestEnv,
    imported: Vec<History>,
) {
    let mut client = env.history_client().await;
    let recorded = env.record(&mut client, "echo recorded").await;
    let running = client.start_history(history("sleep 30")).await.unwrap().id.unwrap();
    env.history_db.save_bulk(&imported).await.unwrap();

    client.reload_history().await.unwrap();

    assert_eq!(env.index_hits("recorded").await, vec![recorded]);
    assert!(env.journal.get(running.try_into().unwrap()).is_ok());
    assert_eq!(env.index_count().await, 1 + imported.len());
}
