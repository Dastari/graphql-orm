async fn plain_async() -> u32 { must_be_removed() }
fn future() -> impl core::future::Future<Output = Result<u32, ()>> + Send {
    must_be_removed()
}
fn stream() -> impl futures::Stream<Item = Result<ChangedEvent, ()>> + Send {
    must_be_removed()
}
fn wrapped() -> Result<impl futures::Stream<Item = Result<ChangedEvent, ()>> + Send, ()> {
    must_be_removed()
}
async fn async_wrapped() -> Result<impl futures::Stream<Item = Result<ChangedEvent, ()>> + Send, ()> {
    must_be_removed()
}
fn boxed() -> core::pin::Pin<Box<dyn core::future::Future<Output = ()>>> {
    must_be_removed()
}
