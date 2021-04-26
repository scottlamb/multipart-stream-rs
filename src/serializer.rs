// Copyright (C) 2021 Scott Lamb <slamb@slamb.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Serializes [crate::Part]s into a byte stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{BufMut, Bytes, BytesMut};
use futures::Stream;
use http::HeaderMap;
use pin_project::pin_project;

use crate::Part;

/// Serializes [Part]s into [Bytes].
/// Sets the `Content-Length` header on each part rather than expecting the caller to do so.
pub fn serialize<S, E>(parts: S, boundary: &str) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Part, E>>,
{
    let mut b = BytesMut::with_capacity(boundary.len() + 4);
    b.put(&b"--"[..]);
    b.put(boundary.as_bytes());
    b.put(&b"\r\n"[..]);

    Serializer {
        parts,
        boundary: b.freeze(),
        state: State::Waiting,
    }
}

/// Serializes HTTP headers into the usual form, including a final empty line.
fn serialize_headers(headers: HeaderMap) -> Bytes {
    // This is the same reservation hyper uses. It calls it "totally scientific".
    let mut b = BytesMut::with_capacity(30 + 30 * headers.len());
    for (name, value) in &headers {
        b.put(name.as_str().as_bytes());
        b.put(&b": "[..]);
        b.put(value.as_bytes());
        b.put(&b"\r\n"[..]);
    }
    b.put(&b"\r\n"[..]);
    b.freeze()
}

/// State of the [Serializer].
enum State {
    /// Waiting for a fresh [Part] from the inner stream.
    Waiting,

    /// Waiting for a chance to send the headers of a previous [Part].
    SendHeaders(Part),

    /// Waiting for a chance to send the body of a previous [Part].
    SendBody(Bytes),
}

#[pin_project]
struct Serializer<S, E>
where
    S: Stream<Item = Result<Part, E>>,
{
    #[pin]
    parts: S,
    boundary: Bytes,
    state: State,
}

impl<S, E> Stream for Serializer<S, E>
where
    S: Stream<Item = Result<Part, E>>,
{
    type Item = Result<Bytes, E>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match std::mem::replace(this.state, State::Waiting) {
            State::Waiting => match this.parts.as_mut().poll_next(ctx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(Some(Ok(mut p))) => {
                    p.headers.insert(
                        http::header::CONTENT_LENGTH,
                        http::HeaderValue::from(p.body.len()),
                    );
                    *this.state = State::SendHeaders(p);
                    return Poll::Ready(Some(Ok(this.boundary.clone())));
                }
            },
            State::SendHeaders(part) => {
                *this.state = State::SendBody(part.body);
                let headers = serialize_headers(part.headers);
                return Poll::Ready(Some(Ok(headers)));
            }
            State::SendBody(body) => {
                return Poll::Ready(Some(Ok(body)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::{BufMut, Bytes, BytesMut};
    use futures::{Stream, StreamExt};
    use http::HeaderMap;

    use super::{serialize, Part};

    async fn collect<S, E>(mut s: S) -> Result<Bytes, E>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
    {
        let mut accum = BytesMut::new();
        while let Some(b) = s.next().await {
            accum.put(b?);
        }
        Ok(accum.freeze())
    }

    #[tokio::test]
    async fn success() {
        let input = futures::stream::iter(vec![
            Ok::<_, std::convert::Infallible>(Part {
                headers: HeaderMap::new(),
                body: "foo".into(),
            }),
            Ok::<_, std::convert::Infallible>(Part {
                headers: HeaderMap::new(),
                body: "bar".into(),
            }),
        ]);
        let collected = collect(serialize(input, "b")).await.unwrap();
        let collected = std::str::from_utf8(&collected[..]).unwrap();
        assert_eq!(
            collected,
            "--b\r\ncontent-length: 3\r\n\r\nfoo\
             --b\r\ncontent-length: 3\r\n\r\nbar"
        );
    }

    #[tokio::test]
    async fn err() {
        let e: Box<dyn std::error::Error + Send + Sync> = "uh-oh".to_owned().into();
        let input = futures::stream::iter(vec![Err(e)]);
        assert_eq!(
            collect(serialize(input, "b"))
                .await
                .unwrap_err()
                .to_string(),
            "uh-oh"
        );
    }
}
