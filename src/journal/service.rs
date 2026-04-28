use crate::messages::{IncomingMessage, OutgoingMessage};

use super::repository::JournalRepository;

#[derive(Debug, Clone)]
pub struct JournalService {
    repository: JournalRepository,
}

impl JournalService {
    pub fn new(repository: JournalRepository) -> Self {
        Self { repository }
    }

    pub async fn process(&self, message: &IncomingMessage) -> Result<OutgoingMessage, sqlx::Error> {
        self.repository.store(message).await?;
        Ok(OutgoingMessage {
            text: "Message saved.".to_string(),
        })
    }
}
