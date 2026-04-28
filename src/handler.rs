use std::{error::Error, future::Future};

use crate::messages::{IncomingMessage, OutgoingMessage};

pub trait MessageHandler: Clone + Send + Sync + 'static {
    fn process(
        &self,
        message: &IncomingMessage,
    ) -> impl Future<Output = Result<OutgoingMessage, Box<dyn Error + Send + Sync>>> + Send;

    fn recent(
        &self,
        user_id: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Option<OutgoingMessage>, Box<dyn Error + Send + Sync>>> + Send;
}
