pub mod telegram;

pub trait Adapter {
    async fn run(self);
}
