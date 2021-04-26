// Copyright (C) 2021 Scott Lamb <slamb@slamb.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Example `hyper`-based HTTP server that generates a multipart stream.

use bytes::Bytes;
use http::{HeaderMap, Request, Response};
use hyper::service::{make_service_fn, service_fn};
use hyper::Body;

type BoxedError = Box<dyn std::error::Error + Send + Sync>;

async fn serve(_req: Request<Body>) -> Result<Response<Body>, BoxedError> {
    let stream = futures::stream::unfold(0, |state| async move {
        let body = Bytes::from(format!("part {}", state));
        let part = multipart_stream::Part {
            headers: HeaderMap::new(),
            body,
        };
        match state {
            10 => None,
            _ => Some((Ok::<_, std::convert::Infallible>(part), state + 1)),
        }
    });
    let stream = multipart_stream::serialize(stream, "foo");

    Ok(hyper::Response::builder()
        .header(http::header::CONTENT_TYPE, "multipart/mixed; boundary=foo")
        .body(hyper::Body::wrap_stream(stream))?)
}

#[tokio::main]
async fn main() {
    let addr = ([127, 0, 0, 1], 0).into();
    let make_svc = make_service_fn(move |_conn| {
        futures::future::ok::<_, std::convert::Infallible>(service_fn(move |req| serve(req)))
    });
    let server = hyper::Server::bind(&addr).serve(make_svc);
    println!("Serving on http://{}", server.local_addr());
    server.await.unwrap();
}
