use http::Response;
use hyper::body::{Body, Bytes, Frame, SizeHint};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::BoxBody;
use tonic::transport::channel::ResponseFuture;
use tonic::transport::{Channel, Error};
use tonic::Status;
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

struct RequestFrameMonitorBody {
    inner: Pin<Box<dyn Body<Data = Bytes, Error = Status> + Send + 'static>>,
    frame_signal: FrameSignal,
}

impl Body for RequestFrameMonitorBody {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(res)) => match res {
                Ok(frame) => {
                    self.frame_signal.signal();
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
pub struct RequestFrameMonitor {
    /// Wrapped channel to monitor.
    inner: Channel,

    /// Signal indicating if request frame has been produced.
    frame_signal: FrameSignal,
}

impl RequestFrameMonitor {
    pub fn new(inner: Channel, frame_signal: FrameSignal) -> Self {
        Self {
            inner,
            frame_signal: frame_signal.clone(),
        }
    }
}

impl Service<http::Request<BoxBody>> for RequestFrameMonitor {
    type Response = Response<BoxBody>;
    type Error = Error;
    type Future = ResponseFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<BoxBody>) -> Self::Future {
        let (head, body) = req.into_parts();
        let body = BoxBody::new(RequestFrameMonitorBody {
            inner: Box::pin(body),
            frame_signal: self.frame_signal.clone(),
        });
        // See <https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services>
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        inner.call(http::Request::from_parts(head, body))
    }
}
