//! `atuin import` hands shell history to the daemon, which adds it to the history db, the record
//! store and the search index, like any command it records itself.
#![cfg(unix)]

mod common;

use atuin_client::history::store::HistoryRecord;
use atuin_client::history::{History, HistoryId};
use atuin_common::utils::uuid_v7;
use common::{TestEnv, history_at};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

/// Finished entries as an importer produces them, one second apart so none collide in the db.
#[fixture]
fn imported() -> Vec<History> {
    let start = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    (0..3)
        .map(|i| History {
            duration: 1_000,
            ..history_at(&format!("echo imported {i}"), start + time::Duration::seconds(i))
        })
        .collect()
}

fn sorted_ids<'a>(histories: impl IntoIterator<Item = &'a History>) -> Vec<HistoryId> {
    let mut ids: Vec<_> = histories.into_iter().map(|h| h.id).collect();
    ids.sort();
    ids
}

#[rstest]
#[tokio::test]
async fn import_lands_in_db_store_and_search(#[future(awt)] env: TestEnv, imported: Vec<History>) {
    let mut client = env.history_client().await;
    let mut search = env.search_client().await;

    let reply = client.import_history(imported.clone()).await.unwrap();
    assert_eq!(reply.imported, 3);
    assert_eq!(reply.protocol, 6);

    let mut hits = env.rpc_hits(&mut search, "imported").await;
    hits.sort();
    assert_eq!(hits, sorted_ids(&imported));

    for h in &imported {
        assert_eq!(env.history_db.load(h.id).await.unwrap().as_ref(), Some(h));
    }
    let records: Vec<_> = env.history_records().await;
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|r| matches!(r, HistoryRecord::Create(h) if imported.contains(h))));
}

/// Importing the same shell history again mints fresh ids; the daemon must recognise the entries
/// and leave the db, store and index as they were, apart from genuinely new entries.
#[rstest]
#[tokio::test]
async fn reimport_only_adds_new_entries(#[future(awt)] env: TestEnv, imported: Vec<History>) {
    let mut client = env.history_client().await;
    client.import_history(imported.clone()).await.unwrap();

    let mut reimport: Vec<_> = imported
        .iter()
        .map(|h| History {
            id: HistoryId::from(uuid_v7()),
            ..h.clone()
        })
        .collect();
    let new = History {
        duration: 1_000,
        ..common::history("echo brand new")
    };
    reimport.push(new.clone());

    let reply = client.import_history(reimport).await.unwrap();
    assert_eq!(reply.imported, 1);

    assert_eq!(env.history_records().await.len(), 4);
    assert_eq!(env.active_rows().await, 4);
    assert_eq!(env.index_count().await, 4);
    let mut hits = env.index_hits("imported").await;
    hits.sort();
    assert_eq!(hits, sorted_ids(&imported), "search still points at the original rows");
    assert_eq!(env.index_hits("brand new").await, vec![new.id]);
}

/// One importer batch of long commands (100 x 64 KiB) is past the daemon's 4 MiB message limit; the
/// client must split it rather than fail the import part-way through.
#[rstest]
#[tokio::test]
async fn import_splits_batches_past_the_message_limit(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let start = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    let long: Vec<_> = (0..100)
        .map(|i| History {
            duration: 1_000,
            ..history_at(
                &format!("echo long {i} {}", "x".repeat(64 * 1024)),
                start + time::Duration::seconds(i),
            )
        })
        .collect();

    let reply = client.import_history(long).await.unwrap();
    assert_eq!(reply.imported, 100);
    assert_eq!(env.active_rows().await, 100);
    assert_eq!(env.index_count().await, 100);
}
