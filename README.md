# tonic-side-effect

This crate provides a [tower](https://github.com/tower-rs/tower) service for wrapping a [tonic](https://github.com/hyperium/tonic) [Channel](https://docs.rs/tonic/latest/tonic/transport/struct.Channel.html), which monitors if a request [Body](https://docs.rs/http-body/latest/http_body/trait.Body.html) ever produced a frame before returning an error.

The service communicates whether or not `poll_frame` on the request ever produced data via a `FrameSignal`.

### Why might this be helpful?

It can be useful, when building systems that need to carefully consider potential side-effects from RPCs, to disambiguate between failures which occurred strictly before any data from a request could possibly have been read (as might be the case for a connection refused or DNS error), and failures which occurred after request data may have been consumed (e.g. a connection reset).

A tonic Status represents the mapping of many types of underlying failures into gRPC status, and it's not possible to make hard guarantees about whether any request content was transmitted or not simply by considering the status code.

This service provides more direct access to a boundary that is of particular interest for purposes of reasoning about RPC side-effects (and in a safer way than rooting around in the status' error source chain).