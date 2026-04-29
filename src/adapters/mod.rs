pub mod telegram;

pub trait Adapter {
    fn run(self) -> impl std::future::Future<Output = ()> + Send;
}
