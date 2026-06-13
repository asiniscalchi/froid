use clap::Parser;
use froid::{
    cli::Cli,
    handler::MessageHandler,
    journal::{registry::JournalServiceRegistry, registry::JournalServiceRegistryConfig},
    messages::{IncomingMessage, MessageSource},
};
use tokio_util::sync::CancellationToken;

/// Read every journal entry text stored in a tenant's database file.
async fn entry_texts(db_path: &std::path::Path) -> Vec<String> {
    use sqlx::Row;

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    let rows = sqlx::query("SELECT raw_text FROM journal_entries ORDER BY received_at")
        .fetch_all(&pool)
        .await
        .unwrap();
    rows.into_iter().map(|row| row.get("raw_text")).collect()
}

#[tokio::test]
async fn test_multiuser_database_isolation_and_routing() {
    // 1. Create a unique temporary directory for this test
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_{}", test_id));

    // Ensure the temp directory exists
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    // 2. Parse a test Cli config
    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_telegram_token_123",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
    ])
    .unwrap();

    // No OpenAI key: tests must not construct an embedder or call out.
    let mut config = cli.serve_config().unwrap();
    config.openai_api_key = None;
    let shutdown = CancellationToken::new();

    // 3. Instantiate the registry with the custom base directory
    let registry = JournalServiceRegistry::new(JournalServiceRegistryConfig { config, shutdown })
        .with_base_dir(temp_base_dir.clone());

    // 4. Send message for User A (chat_id: "user_a")
    let msg_a = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "user_a".to_string(),
        source_message_id: "msg_1".to_string(),
        text: "Today was a productive day writing Rust integration tests.".to_string(),
        received_at: chrono::Utc::now(),
    };

    // Route message for User A
    let res_a = registry.process(&msg_a).await;
    assert!(
        res_a.is_ok(),
        "Failed to process message for User A: {:?}",
        res_a.err()
    );

    // Verify User A's physical database file was created
    let db_a_path = temp_base_dir.join("user_user_a.sqlite3");
    assert!(
        db_a_path.exists(),
        "User A database file should exist at {:?}",
        db_a_path
    );

    // 5. Send message for User B (chat_id: "user_b")
    let msg_b = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "user_b".to_string(),
        source_message_id: "msg_2".to_string(),
        text: "I spent the afternoon gardening in the backyard.".to_string(),
        received_at: chrono::Utc::now(),
    };

    // Route message for User B
    let res_b = registry.process(&msg_b).await;
    assert!(
        res_b.is_ok(),
        "Failed to process message for User B: {:?}",
        res_b.err()
    );

    // Verify User B's physical database file was created
    let db_b_path = temp_base_dir.join("user_user_b.sqlite3");
    assert!(
        db_b_path.exists(),
        "User B database file should exist at {:?}",
        db_b_path
    );

    // 6. Verify isolation by inspecting each tenant's physical database.
    let texts_a = entry_texts(&db_a_path).await;
    assert!(
        texts_a
            .iter()
            .any(|t| t.contains("Today was a productive day")),
        "User A's database should contain their own message. Got: {:?}",
        texts_a
    );
    assert!(
        !texts_a.iter().any(|t| t.contains("gardening")),
        "User A's database must NOT contain User B's message. Got: {:?}",
        texts_a
    );

    let texts_b = entry_texts(&db_b_path).await;
    assert!(
        texts_b.iter().any(|t| t.contains("gardening")),
        "User B's database should contain their own message. Got: {:?}",
        texts_b
    );
    assert!(
        !texts_b.iter().any(|t| t.contains("productive day")),
        "User B's database must NOT contain User A's message. Got: {:?}",
        texts_b
    );

    // 7. Verify Startup Database Discovery
    // Create a new registry pointing to the same directory
    // This simulates starting up Froid with existing tenant databases on disk.
    let mut restart_config = cli.serve_config().unwrap();
    restart_config.openai_api_key = None;
    let registry_restart = JournalServiceRegistry::new(JournalServiceRegistryConfig {
        config: restart_config,
        shutdown: CancellationToken::new(),
    })
    .with_base_dir(temp_base_dir.clone());

    // Run discovery
    let discovery_res = registry_restart.discover_and_register_existing().await;
    assert!(
        discovery_res.is_ok(),
        "Failed to run database discovery: {:?}",
        discovery_res.err()
    );

    // The restarted registry must route to User A's existing database: a new
    // message lands alongside the entry stored before the restart rather than
    // in a fresh database.
    let msg_a_again = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "user_a".to_string(),
        source_message_id: "msg_3".to_string(),
        text: "A second entry written after restart.".to_string(),
        received_at: chrono::Utc::now(),
    };
    registry_restart.process(&msg_a_again).await.unwrap();

    let texts_a_after_restart = entry_texts(&db_a_path).await;
    assert!(
        texts_a_after_restart
            .iter()
            .any(|t| t.contains("Today was a productive day")),
        "Restarted registry should keep User A's original entry. Got: {:?}",
        texts_a_after_restart
    );
    assert!(
        texts_a_after_restart
            .iter()
            .any(|t| t.contains("second entry written after restart")),
        "Restarted registry should append to User A's existing database. Got: {:?}",
        texts_a_after_restart
    );

    // Clean up temporary database files
    let _ = tokio::fs::remove_dir_all(&temp_base_dir).await;
}

#[tokio::test]
async fn test_multiuser_whitelist_gatekeeping() {
    use froid::workers::review_delivery::ReviewSender;
    use froid::workers::{ReviewSendOutcome, TelegramReviewSender};

    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_whitelist_{}", test_id));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    // Parse config with allowed user IDs set to 12345 and 67890
    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_token",
        "--telegram-allowed-user-ids",
        "12345,67890",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
    ])
    .unwrap();

    let config = cli.serve_config().unwrap();
    assert_eq!(config.telegram_allowed_user_ids, Some(vec![12345, 67890]));

    let sender =
        TelegramReviewSender::new(config.telegram_bot_token, config.telegram_allowed_user_ids);

    // Delivery to non-whitelisted user "99999" must be Skipped (ReviewSendOutcome::Skipped)
    let skipped_daily = sender
        .send_review("daily review", "99999", "test")
        .await
        .unwrap();
    assert_eq!(skipped_daily, ReviewSendOutcome::Skipped);

    let skipped_weekly = sender
        .send_review("weekly review", "99999", "test")
        .await
        .unwrap();
    assert_eq!(skipped_weekly, ReviewSendOutcome::Skipped);

    // Clean up
    let _ = tokio::fs::remove_dir_all(&temp_base_dir).await;
}
