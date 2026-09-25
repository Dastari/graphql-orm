//! Keep the provider's in-flight persistence polled while a renewal waits for it.
use std::{future::Future, pin::Pin};

pub(crate) async fn poll_provider_during_maintenance<P, M>(
    mut provider: Pin<&mut P>,
    maintenance: M,
) -> (M::Output, Option<P::Output>)
where
    P: Future + ?Sized,
    M: Future,
{
    tokio::pin!(maintenance);
    tokio::select! {
        result = &mut maintenance => (result, None),
        result = &mut provider => {
            // A renewal may already have committed. Do not drop it and lose
            // its updated row-version proof when the provider finishes first.
            (maintenance.await, Some(result))
        }
    }
}
