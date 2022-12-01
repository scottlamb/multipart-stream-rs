// Copyright (C) 2021 Scott Lamb <slamb@slamb.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

// This implementation is gross (it's hard to read and copies when not
// necessary), I think due to a combination of the following:
//
// 1.  the current state of Rust async: in particular, that there are no coroutines.
// 2.  my inexperience with Rust async
// 3.  how quickly I threw this together.
//
// Fortunately the badness is hidden behind a decent interface, and there are decent tests
// of success cases with partial data. In the situations we're using it (small
// bits of metadata rather than video), the inefficient probably doesn't matter.
// TODO: add tests of bad inputs.

//! Parses a [`Bytes`] stream into a [`Part`] stream.

use crate::Part;
use bytes::{Buf, Bytes, BytesMut};
use futures::Stream;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use httparse;
use pin_project::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};

/// An error when reading from the underlying stream or parsing.
///
/// When the error comes from the underlying stream, it can be examined via
/// [`std::error::Error::source`].
#[derive(Debug)]
pub struct Error(ErrorInt);

#[derive(Debug)]
enum ErrorInt {
    ParseError(String),
    Underlying(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            ErrorInt::ParseError(ref s) => f.pad(s),
            ErrorInt::Underlying(ref e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            ErrorInt::Underlying(e) => Some(&**e),
            _ => None,
        }
    }
}

/// Creates a parse error with the specified format string and arguments.
macro_rules! parse_err {
    ($($arg:tt)*) => {
        Error(ErrorInt::ParseError(format!($($arg)*)))
    };
}

/// A parsing stream adapter, constructed via [`parse`] or [`ParserBuilder`].
#[pin_project]
pub struct Parser<S, E>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    #[pin]
    input: S,

    /// The boundary with `--` prefix and `\r\n` suffix.
    boundary: Vec<u8>,
    buf: BytesMut,
    state: State,
    max_header_bytes: usize,
    max_body_bytes: usize,
}

enum State {
    /// Consuming 0 or more `\r\n` pairs, advancing when encountering a byte that doesn't fit that pattern.
    Newlines,

    /// Waiting for the completion of a boundary.
    /// `pos` is the current offset within `boundary_buf`.
    Boundary { pos: usize },

    /// Waiting for a full set of headers.
    Headers,

    /// Waiting for a full body.
    Body { headers: HeaderMap, body_len: usize },

    /// The stream is finished (has already returned an error).
    Done,
}

impl State {
    /// Processes the current buffer contents.
    ///
    /// This reverses the order of the return value so it can return error via `?` and `bail!`.
    /// The caller puts it back into the order expected by `Stream`.
    fn process(
        &mut self,
        boundary: &[u8],
        buf: &mut BytesMut,
        max_header_bytes: usize,
        max_body_bytes: usize,
    ) -> Result<Poll<Option<Part>>, Error> {
        'outer: loop {
            match self {
                State::Newlines => {
                    while buf.len() >= 2 {
                        if &buf[0..2] == b"\r\n" {
                            buf.advance(2);
                        } else {
                            *self = Self::Boundary { pos: 0 };
                            continue 'outer;
                        }
                    }
                    if buf.len() == 1 && buf[0] != b'\r' {
                        *self = Self::Boundary { pos: 0 };
                    } else {
                        return Ok(Poll::Pending);
                    }
                }
                State::Boundary { ref mut pos } => {
                    let len = std::cmp::min(boundary.len() - *pos, buf.len());
                    if buf[0..len] != boundary[*pos..*pos + len] {
                        return Err(parse_err!("bad boundary"));
                    }
                    buf.advance(len);
                    *pos += len;
                    if *pos < boundary.len() {
                        return Ok(Poll::Pending);
                    }
                    *self = State::Headers;
                }
                State::Headers => {
                    let mut raw = [httparse::EMPTY_HEADER; 16];
                    let headers = httparse::parse_headers(&buf, &mut raw)
                        .map_err(|e| parse_err!("Part headers invalid: {}", e))?;
                    match headers {
                        httparse::Status::Complete((body_pos, raw)) => {
                            let mut headers = HeaderMap::with_capacity(raw.len());
                            for h in raw {
                                headers.append(
                                    HeaderName::from_bytes(h.name.as_bytes())
                                        .map_err(|_| parse_err!("bad header name"))?,
                                    HeaderValue::from_bytes(h.value)
                                        .map_err(|_| parse_err!("bad header value"))?,
                                );
                            }
                            buf.advance(body_pos);
                            let body_len: usize = headers
                                .get(header::CONTENT_LENGTH)
                                .unwrap_or(&HeaderValue::from(0))
                                .to_str()
                                .map_err(|_| parse_err!("Part Content-Length is not valid string"))?
                                .parse()
                                .map_err(|_| {
                                    parse_err!("Part Content-Length is not valid usize")
                                })?;
                            if body_len > max_body_bytes {
                                return Err(parse_err!(
                                    "body byte length {} exceeds maximum of {}",
                                    body_len,
                                    max_body_bytes
                                ));
                            }
                            *self = State::Body { headers, body_len };
                        }
                        httparse::Status::Partial => {
                            if buf.len() >= max_header_bytes {
                                return Err(parse_err!(
                                    "incomplete {}-byte header, vs maximum of {} bytes",
                                    buf.len(),
                                    max_header_bytes
                                ));
                            }
                            return Ok(Poll::Pending);
                        }
                    }
                }
                State::Body { headers, ref mut body_len } => {
                    if *body_len == 0 {
                        if let Some(n) = memchr::memmem::find(buf, boundary) {
                            *body_len = n;
                        }
                    }
                    if buf.len() >= *body_len && *body_len > 0 {
                        let body = buf.split_to(*body_len).freeze();
                        let headers = std::mem::replace(headers, HeaderMap::new());
                        *self = State::Newlines;
                        return Ok(Poll::Ready(Some(Part { headers, body })));
                    }
                    return Ok(Poll::Pending);
                }
                State::Done => return Ok(Poll::Ready(None)),
            }
        }
    }
}

/// Flexible builder for [`Parser`].
pub struct ParserBuilder {
    max_header_bytes: usize,
    max_body_bytes: usize,
}

impl ParserBuilder {
    pub fn new() -> Self {
        ParserBuilder {
            max_header_bytes: usize::MAX,
            max_body_bytes: usize::MAX,
        }
    }

    /// Causes the parser to return error if a part's headers exceed this byte length.
    ///
    /// Implementation note: currently this is only checked when about to wait for another chunk.
    /// If a single chunk contains a complete header, it may be parsed successfully in spite of exceeding this length.
    pub fn max_header_bytes(self, max_header_bytes: usize) -> Self {
        ParserBuilder {
            max_header_bytes,
            ..self
        }
    }

    /// Causes the parser to return error if a part's body exceeds this byte length.
    pub fn max_body_bytes(self, max_body_bytes: usize) -> Self {
        ParserBuilder {
            max_body_bytes,
            ..self
        }
    }

    /// Parses a [`Bytes`] stream into a [`Part`] stream.
    ///
    /// `boundary` should be as in the `boundary` parameter of the `Content-Type` header.
    pub fn parse<S, E>(self, input: S, boundary: &str) -> impl Stream<Item = Result<Part, Error>>
    where
        S: Stream<Item = Result<Bytes, E>>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let boundary = {
            let mut line = Vec::with_capacity(boundary.len() + 4);
            if !boundary.starts_with("--") {
                line.extend_from_slice(b"--");
            }
            line.extend_from_slice(boundary.as_bytes());
            line.extend_from_slice(b"\r\n");
            line
        };

        Parser {
            input,
            buf: BytesMut::new(),
            boundary,
            state: State::Newlines,
            max_header_bytes: self.max_header_bytes,
            max_body_bytes: self.max_body_bytes,
        }
    }
}

/// Parses a [`Bytes`] stream into a [`Part`] stream.
///
/// `boundary` should be as in the `boundary` parameter of the `Content-Type` header.
///
/// This doesn't allow customizing the parser; use [`ParserBuilder`] instead if desired.
pub fn parse<S, E>(input: S, boundary: &str) -> impl Stream<Item = Result<Part, Error>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    ParserBuilder::new().parse(input, boundary)
}

impl<S, E> Stream for Parser<S, E>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<Part, Error>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match this.state.process(
                &this.boundary,
                this.buf,
                *this.max_header_bytes,
                *this.max_body_bytes,
            ) {
                Err(e) => {
                    *this.state = State::Done;
                    return Poll::Ready(Some(Err(e.into())));
                }
                Ok(Poll::Ready(Some(r))) => return Poll::Ready(Some(Ok(r))),
                Ok(Poll::Ready(None)) => return Poll::Ready(None),
                Ok(Poll::Pending) => {}
            }
            match this.input.as_mut().poll_next(ctx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    if !matches!(*this.state, State::Newlines) {
                        *this.state = State::Done;
                        return Poll::Ready(Some(Err(parse_err!("unexpected mid-part EOF"))));
                    }
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Err(e))) => {
                    *this.state = State::Done;
                    return Poll::Ready(Some(Err(Error(ErrorInt::Underlying(e.into())))));
                }
                Poll::Ready(Some(Ok(b))) => {
                    this.buf.extend_from_slice(&b);
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, ParserBuilder, Part};
    use bytes::Bytes;
    use futures::StreamExt;

    /// Tries parsing `input` with a stream that has chunks of different sizes arriving.
    ///
    /// This ensures that the "not enough data for the current state", "enough for the current state
    /// exactly", "enough for the current state and some for the next", and "enough for the next
    /// state (and beyond)" cases are exercised.
    async fn tester<F>(boundary: &str, input: &'static [u8], verify_parts: F)
    where
        F: Fn(Vec<Result<Part, Error>>),
    {
        for chunk_size in &[1, 2, usize::MAX] {
            let input: Vec<Result<Bytes, std::convert::Infallible>> = input
                .chunks(*chunk_size)
                .map(|c: &[u8]| Ok(Bytes::from(c)))
                .collect();
            let input = futures::stream::iter(input);
            let parts = ParserBuilder::new().parse(input, boundary);
            let output_stream: Vec<Result<Part, Error>> = parts.collect().await;
            verify_parts(output_stream);
        }
    }

    #[tokio::test]
    async fn truncated_header() {
        let input = "--boundary\r\nPartial-Header";
        let verify_parts = |mut parts: Vec<Result<Part, Error>>| {
            assert_eq!(parts.len(), 1);
            parts.pop().unwrap().unwrap_err();
        };
        tester("boundary", input.as_bytes(), verify_parts).await;
        tester("--boundary", input.as_bytes(), verify_parts).await;
    }

    #[tokio::test]
    async fn truncated_data() {
        let input = "--boundary\r\nContent-Length: 42\r\n\r\n";
        let verify_parts = |mut parts: Vec<Result<Part, Error>>| {
            assert_eq!(parts.len(), 1);
            parts.pop().unwrap().unwrap_err();
        };
        tester("boundary", input.as_bytes(), verify_parts).await;
        tester("--boundary", input.as_bytes(), verify_parts).await;
    }

    #[tokio::test]
    async fn hikvision_style() {
        let input = concat!(
            "--boundary\r\n",
            "Content-Type: application/xml; charset=\"UTF-8\"\r\n",
            "Content-Length: 480\r\n",
            "\r\n",
            "<EventNotificationAlert version=\"1.0\" ",
            "xmlns=\"http://www.hikvision.com/ver10/XMLSchema\">\r\n",
            "<ipAddress>192.168.5.106</ipAddress>\r\n",
            "<portNo>80</portNo>\r\n",
            "<protocol>HTTP</protocol>\r\n",
            "<macAddress>8c:e7:48:da:94:8f</macAddress>\r\n",
            "<channelID>1</channelID>\r\n",
            "<dateTime>2019-02-20T15:22:34-8:00</dateTime>\r\n",
            "<activePostCount>0</activePostCount>\r\n",
            "<eventType>videoloss</eventType>\r\n",
            "<eventState>inactive</eventState>\r\n",
            "<eventDescription>videoloss alarm</eventDescription>\r\n",
            "</EventNotificationAlert>\r\n",
            "--boundary\r\n",
            "Content-Type: application/xml; charset=\"UTF-8\"\r\n",
            "Content-Length: 480\r\n",
            "\r\n",
            "<EventNotificationAlert version=\"1.0\" ",
            "xmlns=\"http://www.hikvision.com/ver10/XMLSchema\">\r\n",
            "<ipAddress>192.168.5.106</ipAddress>\r\n",
            "<portNo>80</portNo>\r\n",
            "<protocol>HTTP</protocol>\r\n",
            "<macAddress>8c:e7:48:da:94:8f</macAddress>\r\n",
            "<channelID>1</channelID>\r\n",
            "<dateTime>2019-02-20T15:22:34-8:00</dateTime>\r\n",
            "<activePostCount>0</activePostCount>\r\n",
            "<eventType>videoloss</eventType>\r\n",
            "<eventState>inactive</eventState>\r\n",
            "<eventDescription>videoloss alarm</eventDescription>\r\n",
            "</EventNotificationAlert>\r\n"
        );

        let verify_parts = |parts: Vec<Result<Part, Error>>| {
            let mut i = 0;
            for p in parts {
                let p = p.unwrap();
                assert_eq!(
                    p.headers
                        .get(http::header::CONTENT_TYPE)
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    "application/xml; charset=\"UTF-8\""
                );
                assert!(p.body.starts_with(b"<EventNotificationAlert"));
                assert!(p.body.ends_with(b"</EventNotificationAlert>\r\n"));
                i += 1;
            }
            assert_eq!(i, 2);
        };
        tester("boundary", input.as_bytes(), verify_parts).await;
        tester("--boundary", input.as_bytes(), verify_parts).await;
    }

    #[tokio::test]
    async fn dahua_style() {
        let input = concat!(
            "--myboundary\r\n",
            "Content-Type: text/plain\r\n",
            "Content-Length:135\r\n",
            "\r\n",
            "Code=TimeChange;action=Pulse;index=0;data={\n",
            "   \"BeforeModifyTime\" : \"2019-02-20 13:49:58\",\n",
            "   \"ModifiedTime\" : \"2019-02-20 13:49:58\"\n",
            "}\n",
            "\r\n",
            "\r\n",
            "--myboundary\r\n",
            "Content-Type: text/plain\r\n",
            "Content-Length:137\r\n",
            "\r\n",
            "Code=NTPAdjustTime;action=Pulse;index=0;data={\n",
            "   \"Address\" : \"192.168.5.254\",\n",
            "   \"Before\" : \"2019-02-20 13:49:57\",\n",
            "   \"result\" : true\n",
            "}\n\r\n"
        );
        let verify_parts = |parts: Vec<Result<Part, Error>>| {
            let mut i = 0;
            for p in parts {
                let p = p.unwrap();
                assert_eq!(
                    p.headers
                        .get(http::header::CONTENT_TYPE)
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    "text/plain"
                );
                match i {
                    0 => assert!(p.body.starts_with(b"Code=TimeChange")),
                    1 => assert!(p.body.starts_with(b"Code=NTPAdjustTime")),
                    _ => unreachable!(),
                }
                i += 1;
            }
            assert_eq!(i, 2);
        };
        tester("myboundary", input.as_bytes(), verify_parts).await;
        tester("--myboundary", input.as_bytes(), verify_parts).await;
    }

    #[tokio::test]
    async fn dahua_heartbeat() {
        // Dahua event streams have a `heartbeat` parameter which sends messages like the one below.
        // The newlines are before the part, rather than after the part as in other messages.
        // The heartbeat is sometimes the first message in the stream. We need to allow initial
        // newlines to avoid erroring in this case.
        let input = concat!(
            "\r\n--myboundary\r\n",
            "Content-Type: text/plain\r\n",
            "Content-Length:9\r\n\r\n",
            "Heartbeat"
        );
        let verify_parts = |parts: Vec<Result<Part, Error>>| {
            let mut i = 0;
            for p in parts {
                let p = p.unwrap();
                assert_eq!(&p.body[..], b"Heartbeat");
                i += 1;
            }
            assert_eq!(i, 1);
        };
        tester("myboundary", input.as_bytes(), verify_parts).await;
        tester("--myboundary", input.as_bytes(), verify_parts).await;
    }
}
