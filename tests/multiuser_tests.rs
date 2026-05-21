use tokio_util::sync::CancellationToken;
use froid::{
    cli::Cli,
    journal::{
        registry::JournalServiceRegistry,
        extraction::JournalEntryExtractionRuntimeConfig,
        review::DailyReviewRuntimeConfig,
        week_review::WeeklyReviewRuntimeConfig,
        review::signals::wiring::DailyReviewSignalRuntimeConfig,
    },
    handler::MessageHandler,
    messages::{IncomingMessage, MessageSource, SINGLE_USER_ID},
    journal::command::{JournalCommand, JournalCommandRequest},
};
use clap::Parser;

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
    
    let config = cli.serve_config().unwrap();

    // Setup basic configs
    let embedding_config = None; // No OpenAI embedder for tests to avoid API calls
    let daily_review_config = DailyReviewRuntimeConfig::from_env();
    let weekly_review_config = WeeklyReviewRuntimeConfig::from_env();
    let entry_extraction_config = JournalEntryExtractionRuntimeConfig::from_env();
    let signal_runtime_config = DailyReviewSignalRuntimeConfig::from_env();
    let shutdown = CancellationToken::new();

    // 3. Instantiate the registry with the custom base directory
    let registry = JournalServiceRegistry::new(
        config,
        embedding_config,
        entry_extraction_config,
        daily_review_config,
        weekly_review_config,
        signal_runtime_config,
        false, // delivery_configured
        shutdown,
    )
    .with_base_dir(temp_base_dir.clone());

    // 4. Send message for User A (chat_id: "user_a")
    let msg_a = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "user_a".to_string(),
        source_message_id: "msg_1".to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        text: "Today was a productive day writing Rust integration tests.".to_string(),
        received_at: chrono::Utc::now(),
    };

    // Route message for User A
    let res_a = registry.process(&msg_a).await;
    assert!(res_a.is_ok(), "Failed to process message for User A: {:?}", res_a.err());

    // Verify User A's physical database file was created
    let db_a_path = temp_base_dir.join("user_user_a.sqlite3");
    assert!(db_a_path.exists(), "User A database file should exist at {:?}", db_a_path);

    // 5. Send message for User B (chat_id: "user_b")
    let msg_b = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "user_b".to_string(),
        source_message_id: "msg_2".to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        text: "I spent the afternoon gardening in the backyard.".to_string(),
        received_at: chrono::Utc::now(),
    };

    // Route message for User B
    let res_b = registry.process(&msg_b).await;
    assert!(res_b.is_ok(), "Failed to process message for User B: {:?}", res_b.err());

    // Verify User B's physical database file was created
    let db_b_path = temp_base_dir.join("user_user_b.sqlite3");
    assert!(db_b_path.exists(), "User B database file should exist at {:?}", db_b_path);

    // 6. Verify Isolation via /recent command
    // Query recent entries for User A
    let cmd_a = JournalCommandRequest {
        source: MessageSource::Telegram,
        source_conversation_id: "user_a".to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        received_at: chrono::Utc::now(),
        command: JournalCommand::Recent { requested_limit: 10 },
    };
    
    let res_recent_a = registry.command(&cmd_a).await.unwrap();
    assert!(
        res_recent_a.text.contains("Today was a productive day"),
        "User A's recent list should contain their own message. Got: {}",
        res_recent_a.text
    );
    assert!(
        !res_recent_a.text.contains("gardening"),
        "User A's recent list must NOT contain User B's message. Got: {}",
        res_recent_a.text
    );

    // Query recent entries for User B
    let cmd_b = JournalCommandRequest {
        source: MessageSource::Telegram,
        source_conversation_id: "user_b".to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        received_at: chrono::Utc::now(),
        command: JournalCommand::Recent { requested_limit: 10 },
    };

    let res_recent_b = registry.command(&cmd_b).await.unwrap();
    assert!(
        res_recent_b.text.contains("gardening"),
        "User B's recent list should contain their own message. Got: {}",
        res_recent_b.text
    );
    assert!(
        !res_recent_b.text.contains("productive day"),
        "User B's recent list must NOT contain User A's message. Got: {}",
        res_recent_b.text
    );

    // 7. Verify Startup Database Discovery
    // Create a new registry pointing to the same directory
    // This simulates starting up Froid with existing tenant databases on disk.
    let registry_restart = JournalServiceRegistry::new(
        cli.serve_config().unwrap(),
        None,
        JournalEntryExtractionRuntimeConfig::from_env(),
        DailyReviewRuntimeConfig::from_env(),
        WeeklyReviewRuntimeConfig::from_env(),
        DailyReviewSignalRuntimeConfig::from_env(),
        false,
        CancellationToken::new(),
    )
    .with_base_dir(temp_base_dir.clone());

    // Run discovery
    let discovery_res = registry_restart.discover_and_register_existing().await;
    assert!(discovery_res.is_ok(), "Failed to run database discovery: {:?}", discovery_res.err());

    // Verify both tenant services were loaded and cached
    // We can query recent again using the restarted registry without sending a new message first
    let res_restart_a = registry_restart.command(&cmd_a).await.unwrap();
    assert!(
        res_restart_a.text.contains("Today was a productive day"),
        "Restarted registry should discover User A's DB and load existing entries. Got: {}",
        res_restart_a.text
    );

    // Clean up temporary database files
    let _ = tokio::fs::remove_dir_all(&temp_base_dir).await;
}

#[tokio::test]
async fn test_multiuser_legacy_compatibility() {
    let test_id = ulid::Ulid::new().to_string();
    let temp_base_dir = std::env::temp_dir().join(format!("froid_test_legacy_{}", test_id));
    tokio::fs::create_dir_all(&temp_base_dir).await.unwrap();

    let legacy_db_file = temp_base_dir.join("legacy_froid.sqlite3");

    // Parse config with allowed user ID set to 99999
    let cli = Cli::try_parse_from([
        "froid",
        "--telegram-bot-token",
        "mock_token",
        "--telegram-allowed-user-id",
        "99999",
        "--data-dir",
        temp_base_dir.to_str().unwrap(),
        "--database-file",
        "legacy_froid.sqlite3",
    ])
    .unwrap();

    let config = cli.serve_config().unwrap();

    let registry = JournalServiceRegistry::new(
        config,
        None,
        JournalEntryExtractionRuntimeConfig::from_env(),
        DailyReviewRuntimeConfig::from_env(),
        WeeklyReviewRuntimeConfig::from_env(),
        DailyReviewSignalRuntimeConfig::from_env(),
        false,
        CancellationToken::new(),
    )
    .with_base_dir(temp_base_dir.clone());

    // When chat_id matches allowed_user_id "99999", it should route to the legacy database file path
    let msg = IncomingMessage {
        source: MessageSource::Telegram,
        source_conversation_id: "99999".to_string(),
        source_message_id: "msg_legacy".to_string(),
        user_id: SINGLE_USER_ID.to_string(),
        text: "This entry goes to the legacy file database.".to_string(),
        received_at: chrono::Utc::now(),
    };

    let res = registry.process(&msg).await;
    assert!(res.is_ok());

    // Verify the entry was written to the legacy path
    assert!(legacy_db_file.exists(), "Legacy database file should be created at {:?}", legacy_db_file);

    // Verify it did NOT create the isolated tenant path
    let isolated_path = temp_base_dir.join("user_99999.sqlite3");
    assert!(!isolated_path.exists(), "Isolated database file user_99999.sqlite3 should NOT exist since allowed_user_id was mapped to legacy path");

    // Clean up
    let _ = tokio::fs::remove_dir_all(&temp_base_dir).await;
}
