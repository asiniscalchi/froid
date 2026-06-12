use chrono::{TimeZone, Utc};
use sqlx::{Row, SqlitePool};

use super::repository::*;
use crate::database;
use crate::messages::{IncomingMessage, MessageSource};

async fn setup() -> JournalRepository {
    database::register_sqlite_vec_extension();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    JournalRepository::new(pool)
}

fn incoming(
    source_message_id: &str,
    text: &str,
    received_at: chrono::DateTime<Utc>,
) -> IncomingMessage {
    incoming_for_conversation("42", source_message_id, text, received_at)
}

fn incoming_for_conversation(
    source_conversation_id: &str,
    source_message_id: &str,
    text: &str,
    received_at: chrono::DateTime<Utc>,
) -> IncomingMessage {
    IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: source_conversation_id.to_string(),
        source_message_id: source_message_id.to_string(),
        text: text.to_string(),
        received_at,
    }
}

fn at(h: u32, m: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 28, h, m, 0).unwrap()
}

fn date() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()
}

#[tokio::test]
async fn stores_incoming_message() {
    let repo = setup().await;
    let message = incoming("100", "hello froid", Utc::now());

    let journal_entry_id = repo.store(&message).await.unwrap();

    let row = sqlx::query(
        "SELECT id, source, source_conversation_id, source_message_id, raw_text FROM journal_entries",
    )
    .fetch_one(repo.pool())
    .await
    .unwrap();

    assert_eq!(journal_entry_id, Some(row.get("id")));
    assert_eq!(row.get::<String, _>("source"), "telegram");
    assert_eq!(row.get::<String, _>("source_conversation_id"), "42");
    assert_eq!(row.get::<String, _>("source_message_id"), "100");
    assert_eq!(row.get::<String, _>("raw_text"), "hello froid");
}

#[tokio::test]
async fn ignores_duplicate_source_message() {
    let repo = setup().await;
    let message = incoming("100", "hello froid", Utc::now());

    let first = repo.store(&message).await.unwrap();
    let second = repo.store(&message).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(repo.pool())
        .await
        .unwrap();

    assert!(first.is_some());
    assert_eq!(second, None);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn stores_different_messages_independently() {
    let repo = setup().await;

    repo.store(&incoming("100", "hello froid", Utc::now()))
        .await
        .unwrap();
    repo.store(&incoming("101", "hello froid", Utc::now()))
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journal_entries")
        .fetch_one(repo.pool())
        .await
        .unwrap();

    assert_eq!(count, 2);
}

#[tokio::test]
async fn fetch_recent_returns_entries_newest_first() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "third", at(12, 0)))
        .await
        .unwrap();

    let entries = repo.fetch_recent(10).await.unwrap();

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].entry.text, "third");
    assert_eq!(entries[1].entry.text, "second");
    assert_eq!(entries[2].entry.text, "first");
}

#[tokio::test]
async fn fetch_all_returns_every_entry_newest_first() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "third", at(12, 0)))
        .await
        .unwrap();

    let entries = repo.fetch_all().await.unwrap();

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].entry.text, "third");
    assert_eq!(entries[1].entry.text, "second");
    assert_eq!(entries[2].entry.text, "first");
}

#[tokio::test]
async fn fetch_all_for_export_returns_full_records() {
    let repo = setup().await;
    repo.store(&incoming("1", "hi", at(10, 0))).await.unwrap();

    let rows = repo.fetch_all_for_export().await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, "telegram");
    assert_eq!(rows[0].source_conversation_id, "42");
    assert_eq!(rows[0].source_message_id, "1");
    assert_eq!(rows[0].text, "hi");
}

#[tokio::test]
async fn bulk_import_inserts_all_records() {
    let repo = setup().await;

    let records = vec![
        JournalEntryRecord {
            id: String::new(),
            source: "telegram".to_string(),
            source_conversation_id: "42".to_string(),
            source_message_id: "imp-1".to_string(),
            text: "imported one".to_string(),
            received_at: at(10, 0),
        },
        JournalEntryRecord {
            id: String::new(),
            source: "telegram".to_string(),
            source_conversation_id: "42".to_string(),
            source_message_id: "imp-2".to_string(),
            text: "imported two".to_string(),
            received_at: at(11, 0),
        },
    ];

    let inserted = repo.bulk_import(&records).await.unwrap();
    assert_eq!(inserted, 2);

    let entries = repo.fetch_recent(10).await.unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn bulk_import_rolls_back_on_unique_violation() {
    let repo = setup().await;
    repo.store(&incoming("dup", "existing", at(10, 0)))
        .await
        .unwrap();

    let records = vec![
        JournalEntryRecord {
            id: String::new(),
            source: "telegram".to_string(),
            source_conversation_id: "42".to_string(),
            source_message_id: "fresh".to_string(),
            text: "fresh entry".to_string(),
            received_at: at(11, 0),
        },
        JournalEntryRecord {
            id: String::new(),
            source: "telegram".to_string(),
            source_conversation_id: "42".to_string(),
            source_message_id: "dup".to_string(),
            text: "collides".to_string(),
            received_at: at(12, 0),
        },
    ];

    let err = repo.bulk_import(&records).await.unwrap_err();
    match err {
        BulkImportError::Conflict {
            source,
            source_conversation_id,
            source_message_id,
        } => {
            assert_eq!(source, "telegram");
            assert_eq!(source_conversation_id, "42");
            assert_eq!(source_message_id, "dup");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    let entries = repo.fetch_recent(10).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.text, "existing");
}

#[tokio::test]
async fn fetch_all_returns_empty_when_no_entries() {
    let repo = setup().await;

    let entries = repo.fetch_all().await.unwrap();

    assert!(entries.is_empty());
}

#[tokio::test]
async fn fetch_last_for_conversation_returns_latest_entry_for_current_conversation() {
    let repo = setup().await;
    repo.store(&incoming_for_conversation(
        "42",
        "1",
        "current old",
        at(10, 0),
    ))
    .await
    .unwrap();
    repo.store(&incoming_for_conversation(
        "42",
        "2",
        "current new",
        at(11, 0),
    ))
    .await
    .unwrap();
    repo.store(&incoming_for_conversation(
        "99",
        "3",
        "other conversation",
        at(12, 0),
    ))
    .await
    .unwrap();

    let entry = repo
        .fetch_last_for_conversation(&MessageSource::Telegram, "42")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(entry.entry.text, "current new");
}

#[tokio::test]
async fn fetch_last_for_conversation_breaks_timestamp_ties_by_id() {
    let repo = setup().await;
    repo.store(&incoming("1", "first inserted", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second inserted", at(10, 0)))
        .await
        .unwrap();

    let entry = repo
        .fetch_last_for_conversation(&MessageSource::Telegram, "42")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(entry.entry.text, "second inserted");
}

#[tokio::test]
async fn delete_last_for_conversation_deletes_same_entry_selected_by_fetch_last() {
    let repo = setup().await;
    repo.store(&incoming("1", "first inserted", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second inserted", at(10, 0)))
        .await
        .unwrap();

    let fetched = repo
        .fetch_last_for_conversation(&MessageSource::Telegram, "42")
        .await
        .unwrap()
        .unwrap();
    let deleted = repo
        .delete_last_for_conversation(&MessageSource::Telegram, "42")
        .await
        .unwrap()
        .unwrap();
    let remaining = repo.fetch_recent(10).await.unwrap();

    assert_eq!(deleted.id, fetched.id);
    assert_eq!(deleted.entry.text, "second inserted");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].entry.text, "first inserted");
}

#[tokio::test]
async fn delete_last_for_conversation_does_not_delete_other_conversations() {
    let repo = setup().await;
    repo.store(&incoming_for_conversation("42", "1", "current", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming_for_conversation("99", "2", "other", at(11, 0)))
        .await
        .unwrap();

    let deleted = repo
        .delete_last_for_conversation(&MessageSource::Telegram, "42")
        .await
        .unwrap()
        .unwrap();
    let other = repo
        .fetch_last_for_conversation(&MessageSource::Telegram, "99")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(deleted.entry.text, "current");
    assert_eq!(other.entry.text, "other");
}

#[tokio::test]
async fn fetch_recent_respects_limit() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "third", at(12, 0)))
        .await
        .unwrap();

    let entries = repo.fetch_recent(2).await.unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "third");
    assert_eq!(entries[1].entry.text, "second");
}

#[tokio::test]
async fn fetch_recent_returns_empty_when_journal_has_no_entries() {
    let repo = setup().await;

    let entries = repo.fetch_recent(10).await.unwrap();

    assert!(entries.is_empty());
}

#[tokio::test]
async fn fetch_today_returns_entries_oldest_first_for_user() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming(
        "3",
        "tomorrow",
        Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
    ))
    .await
    .unwrap();

    let entries = repo.fetch_today(date()).await.unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "first");
    assert_eq!(entries[1].entry.text, "second");
}

fn ymd(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at_on(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
}

#[tokio::test]
async fn fetch_in_range_returns_entries_within_range_newest_first() {
    let repo = setup().await;

    repo.store(&incoming("1", "before", at_on(2026, 4, 27, 23, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "first", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "second", at_on(2026, 4, 28, 12, 0)))
        .await
        .unwrap();
    repo.store(&incoming("4", "after", at_on(2026, 4, 29, 0, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "second");
    assert_eq!(entries[1].entry.text, "first");
}

#[tokio::test]
async fn fetch_in_range_treats_end_date_as_exclusive() {
    let repo = setup().await;

    repo.store(&incoming("1", "midnight start", at_on(2026, 4, 28, 0, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "midnight end", at_on(2026, 4, 29, 0, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
        .await
        .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.text, "midnight start");
}

#[tokio::test]
async fn fetch_in_range_respects_limit() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at_on(2026, 4, 28, 9, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "third", at_on(2026, 4, 28, 11, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 2)
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "third");
    assert_eq!(entries[1].entry.text, "second");
}

#[tokio::test]
async fn fetch_in_range_returns_all_entries_in_single_user_journal() {
    let repo = setup().await;

    repo.store(&incoming("1", "mine", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();
    repo.store(&IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "99".to_string(),
        source_message_id: "2".to_string(),
        text: "theirs".to_string(),
        received_at: at_on(2026, 4, 28, 11, 0),
    })
    .await
    .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "theirs");
    assert_eq!(entries[1].entry.text, "mine");
}

#[tokio::test]
async fn fetch_in_range_returns_empty_when_no_entries_in_range() {
    let repo = setup().await;

    repo.store(&incoming("1", "outside", at_on(2026, 4, 27, 10, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
        .await
        .unwrap();

    assert!(entries.is_empty());
}

#[tokio::test]
async fn fetch_in_range_returns_empty_for_empty_range() {
    let repo = setup().await;

    repo.store(&incoming("1", "entry", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 28), 10)
        .await
        .unwrap();

    assert!(entries.is_empty());
}

#[tokio::test]
async fn fetch_in_range_breaks_timestamp_ties_by_id_desc() {
    let repo = setup().await;

    repo.store(&incoming("1", "first inserted", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second inserted", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();

    let entries = repo
        .fetch_in_range(ymd(2026, 4, 28), ymd(2026, 4, 29), 10)
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "second inserted");
    assert_eq!(entries[1].entry.text, "first inserted");
}

#[tokio::test]
async fn search_text_matches_substring_case_insensitively() {
    let repo = setup().await;

    repo.store(&incoming("1", "I felt anxious before the call", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "calm afternoon", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "ANXIETY again today", at(12, 0)))
        .await
        .unwrap();

    let entries = repo.search_text("ANXI", None, None, 10).await.unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "ANXIETY again today");
    assert_eq!(entries[1].entry.text, "I felt anxious before the call");
}

#[tokio::test]
async fn search_text_returns_results_newest_first() {
    let repo = setup().await;

    repo.store(&incoming("1", "match one", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "match two", at(12, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "match three", at(11, 0)))
        .await
        .unwrap();

    let entries = repo.search_text("match", None, None, 10).await.unwrap();

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].entry.text, "match two");
    assert_eq!(entries[1].entry.text, "match three");
    assert_eq!(entries[2].entry.text, "match one");
}

#[tokio::test]
async fn search_text_respects_limit() {
    let repo = setup().await;

    for i in 0..5u32 {
        repo.store(&incoming(&i.to_string(), &format!("match {i}"), at(10, i)))
            .await
            .unwrap();
    }

    let entries = repo.search_text("match", None, None, 2).await.unwrap();

    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn search_text_returns_matches_from_the_single_user_journal() {
    let repo = setup().await;

    repo.store(&incoming("1", "mine matches", at(10, 0)))
        .await
        .unwrap();
    repo.store(&IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "99".to_string(),
        source_message_id: "2".to_string(),
        text: "theirs matches too".to_string(),
        received_at: at(11, 0),
    })
    .await
    .unwrap();

    let entries = repo.search_text("matches", None, None, 10).await.unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry.text, "theirs matches too");
    assert_eq!(entries[1].entry.text, "mine matches");
}

#[tokio::test]
async fn search_text_filters_by_date_range_with_exclusive_end() {
    let repo = setup().await;

    repo.store(&incoming("1", "match before", at_on(2026, 4, 27, 10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "match within", at_on(2026, 4, 28, 10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "match boundary", at_on(2026, 4, 29, 0, 0)))
        .await
        .unwrap();

    let entries = repo
        .search_text("match", Some(ymd(2026, 4, 28)), Some(ymd(2026, 4, 29)), 10)
        .await
        .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.text, "match within");
}

#[tokio::test]
async fn search_text_returns_empty_when_no_match() {
    let repo = setup().await;

    repo.store(&incoming("1", "calm afternoon", at(10, 0)))
        .await
        .unwrap();

    let entries = repo.search_text("anxiety", None, None, 10).await.unwrap();

    assert!(entries.is_empty());
}

#[tokio::test]
async fn search_text_matches_any_stored_entry() {
    let repo = setup().await;

    repo.store(&incoming("1", "match", at(10, 0)))
        .await
        .unwrap();

    let entries = repo.search_text("match", None, None, 10).await.unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.text, "match");
}

#[tokio::test]
async fn conversations_with_entries_for_date_returns_distinct_source_conversations() {
    let repo = setup().await;

    repo.store(&incoming_for_conversation("42", "1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming_for_conversation("42", "2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "99".to_string(),
        source_message_id: "3".to_string(),
        text: "other user".to_string(),
        received_at: at(12, 0),
    })
    .await
    .unwrap();
    repo.store(&IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "100".to_string(),
        source_message_id: "4".to_string(),
        text: "tomorrow".to_string(),
        received_at: Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
    })
    .await
    .unwrap();

    let conversations = repo
        .conversations_with_entries_for_date(&MessageSource::Telegram, date())
        .await
        .unwrap();

    assert_eq!(
        conversations,
        vec![
            JournalConversation {
                source_conversation_id: "42".to_string(),
            },
            JournalConversation {
                source_conversation_id: "99".to_string(),
            },
        ]
    );
}

async fn stored_id(repo: &JournalRepository, source_message_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT id FROM journal_entries WHERE source = 'telegram' AND source_message_id = ?",
    )
    .bind(source_message_id)
    .fetch_one(repo.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fetch_by_ids_returns_entries_matching_ids() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming("2", "second", at(11, 0)))
        .await
        .unwrap();
    repo.store(&incoming("3", "third", at(12, 0)))
        .await
        .unwrap();

    let first_id = stored_id(&repo, "1").await;
    let third_id = stored_id(&repo, "3").await;

    let rows = repo
        .fetch_by_ids(&[first_id.clone(), third_id.clone()])
        .await
        .unwrap();

    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(rows.len(), 2);
    assert!(ids.contains(&first_id));
    assert!(ids.contains(&third_id));
}

#[tokio::test]
async fn fetch_by_ids_returns_all_matching_entries_in_single_user_journal() {
    let repo = setup().await;

    repo.store(&incoming("1", "mine", at(10, 0))).await.unwrap();
    let my_id = stored_id(&repo, "1").await;

    let other = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "99".to_string(),
        source_message_id: "2".to_string(),
        text: "theirs".to_string(),
        received_at: at(11, 0),
    };
    repo.store(&other).await.unwrap();
    let other_id = stored_id(&repo, "2").await;

    let rows = repo
        .fetch_by_ids(&[my_id.clone(), other_id.clone()])
        .await
        .unwrap();

    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(rows.len(), 2);
    assert!(ids.contains(&my_id));
    assert!(ids.contains(&other_id));
}

#[tokio::test]
async fn fetch_by_ids_returns_empty_for_empty_id_list() {
    let repo = setup().await;

    let rows = repo.fetch_by_ids(&[]).await.unwrap();

    assert!(rows.is_empty());
}

#[tokio::test]
async fn fetch_by_ids_returns_empty_when_no_ids_match() {
    let repo = setup().await;

    let rows = repo
        .fetch_by_ids(&["nonexistent".to_string()])
        .await
        .unwrap();

    assert!(rows.is_empty());
}

#[tokio::test]
async fn stats_returns_counts_and_latest_timestamp_for_user() {
    let repo = setup().await;

    repo.store(&incoming("1", "first", at(10, 0)))
        .await
        .unwrap();
    repo.store(&incoming(
        "2",
        "tomorrow",
        Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap(),
    ))
    .await
    .unwrap();

    let stats = repo.stats(date()).await.unwrap();

    assert_eq!(stats.total_entries, 2);
    assert_eq!(stats.entries_today, 1);
    assert_eq!(
        stats.latest_received_at,
        Some(Utc.with_ymd_and_hms(2026, 4, 29, 9, 0, 0).unwrap())
    );
}

#[tokio::test]
async fn stats_returns_zeroes_when_journal_has_no_entries() {
    let repo = setup().await;

    let stats = repo.stats(date()).await.unwrap();

    assert_eq!(stats.total_entries, 0);
    assert_eq!(stats.entries_today, 0);
    assert_eq!(stats.latest_received_at, None);
}
