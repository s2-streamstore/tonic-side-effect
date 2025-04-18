use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::Body as TonicBody;
use tonic::transport::Channel;
use tower_service::Service;

/// Resettable handle for indicating if a frame has been produced.
#[derive(Clone, Debug, Default)]
pub struct FrameSignal(Arc<AtomicBool>);

impl FrameSignal {
    fn signal(&self) {
        self.0.store(true, Ordering::Release)
    }

    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn is_signalled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Release)
    }
}

pin_project! {
    struct RequestFrameMonitorBody<B> {
        #[pin]
        inner: B,
        frame_signal: FrameSignal,
    }
}

impl<B> Body for RequestFrameMonitorBody<B>
where
    B: Body,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match this.inner.poll_frame(cx) {
            Poll::Ready(Some(res)) => match res {
                Ok(frame) => {
                    this.frame_signal.signal();
                    Poll::Ready(Some(Ok(frame)))
                }
                Err(status) => Poll::Ready(Some(Err(status))),
            },
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Service for monitoring if an HTTP request frame was ever emitted.
#[derive(Clone, Debug)]
pub struct RequestFrameMonitor<S = Channel>
where
    S: Clone,
{
    /// Wrapped channel to monitor.
    inner: S,

    /// Signal indicating if request frame has been produced.
    frame_signal: FrameSignal,
}

impl<S: Clone> RequestFrameMonitor<S> {
    pub fn new(inner: S, frame_signal: FrameSignal) -> Self {
        Self {
            inner,
            frame_signal: frame_signal.clone(),
        }
    }
}

impl<S> Service<http::Request<TonicBody>> for RequestFrameMonitor<S>
where
    S: Service<http::Request<TonicBody>> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<TonicBody>) -> Self::Future {
        let (head, body) = req.into_parts();
        let body = TonicBody::new(RequestFrameMonitorBody {
            inner: body,
            frame_signal: self.frame_signal.clone(),
        });
        // See <https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services>
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        inner.call(http::Request::from_parts(head, body))
    }
}
