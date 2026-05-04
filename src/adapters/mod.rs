pub mod analyzer_telegram;
pub mod telegram;

pub trait Adapter {
    fn run(self) -> impl std::future::Future<Output = ()> + Send;
}
