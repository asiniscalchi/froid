use crate::messages::{IncomingMessage, OutgoingMessage};

#[derive(Debug, Default, Clone, Copy)]
pub struct EchoService;

impl EchoService {
    pub fn new() -> Self {
        Self
    }

    pub fn echo(&self, message: &IncomingMessage) -> OutgoingMessage {
        OutgoingMessage {
            text: message.text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::messages::MessageSource;

    #[test]
    fn returns_outgoing_message_with_same_text() {
        let service = EchoService::new();
        let incoming = IncomingMessage {
            source: MessageSource::Telegram,
            source_chat_id: "42".to_string(),
            source_message_id: "100".to_string(),
            user_id: "7".to_string(),
            text: "hello froid".to_string(),
            received_at: Utc::now(),
        };

        let outgoing = service.echo(&incoming);

        assert_eq!(outgoing.text, incoming.text);
    }
}
