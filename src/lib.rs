use http::Response;
use hyper::body::{Body, Bytes, Frame, SizeHint};
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::BoxBody;
use tonic::metadata::MetadataValue;
use tonic::transport::channel::ResponseFuture;
use tonic::transport::Channel;
use tonic::Status;
use tower_service::Service;

struct RequestFrameMonitorBody {
    inner: Pin<Box<dyn Body<Data = Bytes, Error = Status> + Send + 'static>>,
    http_frame_produced: Arc<AtomicBool>,
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
                    self.http_frame_produced.store(true, Ordering::Release);
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

#[pin_project]
pub struct RequestFrameMonitorFuture {
    /// Wrapped tonic future.
    #[pin]
    inner: ResponseFuture,

    /// Marker if an HTTP frame has ever been produced from the corresponding `Body`.
    http_frame_produced: Arc<AtomicBool>,

    /// Header key to inject into a returned error `Status` if no frames were produced.
    no_frames_tag: &'static str,
}

impl Future for RequestFrameMonitorFuture {
    type Output = Result<Response<BoxBody>, Status>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.inner.poll(cx) {
            Poll::Ready(Ok(response)) => {
                // Note that `http_frame_produced` value is ignored for any `Ok` response.
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(error)) => {
                let mut status = Status::from_error(error.into());
                if !this.http_frame_produced.load(Ordering::Acquire) {
                    status
                        .metadata_mut()
                        .insert(*this.no_frames_tag, MetadataValue::from_static(""));
                }
                Poll::Ready(Err(status))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Service for monitoring if an HTTP request frame was ever emitted.
#[derive(Clone, Debug)]
pub struct RequestFrameMonitor {
    /// Wrapped `tonic::transport::Channel` to monitor.
    inner: Channel,

    /// Header key to inject into a returned error `Status` if no frames were produced.
    no_frames_tag: &'static str,
}

impl RequestFrameMonitor {
    pub fn new(inner: Channel, no_frames_tag: &'static str) -> Self {
        Self {
            inner,
            no_frames_tag,
        }
    }
}

impl Service<http::Request<BoxBody>> for RequestFrameMonitor {
    type Response = Response<BoxBody>;
    type Error = Status;
    type Future = RequestFrameMonitorFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(|e| Status::from_error(e.into()))
    }

    fn call(&mut self, req: http::Request<BoxBody>) -> Self::Future {
        let (head, body) = req.into_parts();
        let http_frame_produced = Arc::new(AtomicBool::new(false));
        let body = BoxBody::new(RequestFrameMonitorBody {
            inner: Box::pin(body),
            http_frame_produced: http_frame_produced.clone(),
        });
        // See <https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services>
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        RequestFrameMonitorFuture {
            inner: inner.call(http::Request::from_parts(head, body)),
            http_frame_produced,
            no_frames_tag: self.no_frames_tag,
        }
    }
}
