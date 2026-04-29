use std::{error::Error, future::Future};

use crate::{
    journal::command::JournalCommandRequest,
    messages::{IncomingMessage, OutgoingMessage},
};

pub trait MessageHandler: Clone + Send + Sync + 'static {
    fn process(
        &self,
        message: &IncomingMessage,
    ) -> impl Future<Output = Result<OutgoingMessage, Box<dyn Error + Send + Sync>>> + Send;

    fn command(
        &self,
        request: &JournalCommandRequest,
    ) -> impl Future<Output = Result<OutgoingMessage, Box<dyn Error + Send + Sync>>> + Send;
}
