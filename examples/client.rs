// Copyright (C) 2021 Scott Lamb <slamb@slamb.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Example `hyper`-based HTTP client which parses a multipart stream.
//! Run with the URL printed by `cargo run --example server` as its one argument.

use std::convert::TryFrom;

use futures::StreamExt;

#[tokio::main]
async fn main() {
    let url = match std::env::args().nth(1) {
        Some(u) => http::Uri::try_from(u).unwrap(),
        None => {
            eprintln!("Usage: client URL");
            std::process::exit(1);
        }
    };
    let client = hyper::Client::new();
    let res = client.get(url).await.unwrap();
    if !res.status().is_success() {
        eprintln!("HTTP request failed with status {}", res.status());
        std::process::exit(1);
    }
    let content_type: mime::Mime = res
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(content_type.type_(), "multipart");
    let boundary = content_type.get_param(mime::BOUNDARY).unwrap();
    let stream = res.into_body();
    let mut stream = multipart_stream::parse(stream, boundary.as_str());
    while let Some(p) = stream.next().await {
        let p = p.unwrap();
        println!("{:?}", &p);
    }
}
