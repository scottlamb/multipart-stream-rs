// Copyright (C) 2021 Scott Lamb <slamb@slamb.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parser and serializer for async multipart streams.
//! See `README.md` for more information.

use bytes::Bytes;
use http::header::HeaderMap;

pub mod parser;
pub mod serializer;

/// A single part, including its headers and body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Part {
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub use parser::{parse, ParserBuilder};
pub use serializer::serialize;

#[cfg(test)]
mod tests {
    use crate::{parse, serialize, Part};
    use bytes::Bytes;
    use futures::{StreamExt, TryStreamExt};
    use http::HeaderMap;

    #[tokio::test]
    async fn roundtrip() {
        let mut hdrs1 = HeaderMap::new();
        hdrs1.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        hdrs1.insert("X-My-Header1", "part1".parse().unwrap());
        let mut hdrs2 = HeaderMap::new();
        hdrs2.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        hdrs2.insert("X-My-Header2", "part 2".parse().unwrap());
        let input = vec![
            Part {
                headers: hdrs1,
                body: Bytes::from("part 1 body"),
            },
            Part {
                headers: hdrs2,
                body: Bytes::from("this is part 2's body"),
            },
        ];
        let byte_stream = serialize(
            futures::stream::iter(input.clone()).map(|p| Ok::<_, std::convert::Infallible>(p)),
            "my boundary",
        );
        let mut output: Vec<Part> = parse(byte_stream, "my boundary")
            .try_collect()
            .await
            .unwrap();

        // Compare, without the added Content-Length header added by serialize.
        for p in &mut output {
            p.headers.remove(http::header::CONTENT_LENGTH);
        }
        assert_eq!(&input[..], &output[..]);
    }
}
