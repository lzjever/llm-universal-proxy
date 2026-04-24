use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody};
use hyper::body::{Frame, SizeHint};
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone, Debug, Default)]
pub(crate) struct DownstreamCancellation {
    token: CancellationToken,
}

#[derive(Clone, Debug)]
pub(crate) struct DownstreamCancellationHandle {
    token: CancellationToken,
}

pub(crate) fn cancellation_channel() -> (DownstreamCancellationHandle, DownstreamCancellation) {
    let token = CancellationToken::new();
    (
        DownstreamCancellationHandle {
            token: token.clone(),
        },
        DownstreamCancellation { token },
    )
}

pub(crate) fn wrap_body_with_cancellation(
    body: Body,
    cancel_handle: DownstreamCancellationHandle,
) -> Body {
    Body::new(DownstreamCancellationBody::new(body, cancel_handle))
}

impl DownstreamCancellation {
    pub(crate) fn disabled() -> Self {
        Self::default()
    }

    pub(crate) fn child_channel(&self) -> (DownstreamCancellationHandle, DownstreamCancellation) {
        let token = self.token.child_token();
        (
            DownstreamCancellationHandle {
                token: token.clone(),
            },
            DownstreamCancellation { token },
        )
    }

    pub(crate) async fn cancelled(&self) {
        self.token.cancelled().await;
    }
}

impl DownstreamCancellationHandle {
    pub(crate) fn cancel(&self) {
        self.token.cancel();
    }

    pub(crate) fn drop_guard(&self) -> DropGuard {
        self.token.clone().drop_guard()
    }
}

struct DownstreamCancellationBody {
    inner: Body,
    guard: Option<DropGuard>,
}

impl DownstreamCancellationBody {
    fn new(inner: Body, cancel_handle: DownstreamCancellationHandle) -> Self {
        let guard = (!inner.is_end_stream()).then(|| cancel_handle.drop_guard());
        Self { inner, guard }
    }

    fn disarm(&mut self) {
        if let Some(guard) = self.guard.take() {
            let _ = guard.disarm();
        }
    }
}

impl HttpBody for DownstreamCancellationBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(None) => {
                this.disarm();
                Poll::Ready(None)
            }
            poll => poll,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::{cancellation_channel, wrap_body_with_cancellation};
    use axum::body::{to_bytes, Body};
    use bytes::Bytes;
    use futures_util::stream;

    #[tokio::test]
    async fn wrapped_body_drop_cancels_token() {
        let (cancel_handle, cancellation) = cancellation_channel();
        let body = Body::from_stream(stream::iter([Result::<Bytes, std::io::Error>::Ok(
            Bytes::from_static(b"pending"),
        )]));
        let wrapped = wrap_body_with_cancellation(body, cancel_handle);

        drop(wrapped);

        tokio::time::timeout(std::time::Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("dropping the wrapped body should cancel the token");
    }

    #[tokio::test]
    async fn wrapped_body_completion_does_not_cancel_token() {
        let (cancel_handle, cancellation) = cancellation_channel();
        let wrapped = wrap_body_with_cancellation(Body::from("done"), cancel_handle);

        let bytes = to_bytes(wrapped, usize::MAX).await.unwrap();
        assert_eq!(bytes, Bytes::from_static(b"done"));

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                cancellation.cancelled(),
            )
            .await
            .is_err(),
            "reading the full wrapped body should disarm cancellation",
        );
    }
}
