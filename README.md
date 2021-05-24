[![crates.io](https://meritbadge.herokuapp.com/multipart-stream)](https://crates.io/crates/multipart-stream)
[![CI](https://github.com/scottlamb/multipart-stream-rs/workflows/CI/badge.svg)](https://github.com/scottlamb/multipart-stream-rs/actions?query=workflow%3ACI)

Rust library to parse and serialize async `multipart/x-mixed-replace` streams,
suitable for use with `reqwest`, `hyper`, and `tokio`.

Note `multipart/x-mixed-replace` is different than `multipart/form-data`; you
might be interested in the
[`multipart` crate](https://crates.io/crates/multipart) for that.

## What's a multipart stream for?

A multipart stream is a sequence of parts in one HTTP response, each part
having its own headers and body. A stream might last forever, serving parts
that didn't exist at the start of the request. This is a type of "hanging GET"
or [Comet](https://en.wikipedia.org/wiki/Comet_(programming)) request. Each
part might represent the latest state and conceptually replace previous ones,
thus the MIME type `multipart/x-mixed-replace`.

It's a simple HTTP/1.1 way of accomplishing what otherwise might require
fancier server- and client-side technologies, such as:

*   [WebSockets](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API)
*   [HTTP/2 Server Push](https://en.wikipedia.org/wiki/HTTP/2_Server_Push)

Never-ending multipart streams seem popular in the IP camera space:

*   Dahua IP cameras provide a `multipart/x-mixed-replace` stream of events
    such as motion detection changes.
    ([spec](http://www.telecamera.ru/bitrix/components/bitrix/forum.interface/show_file.php?fid=1022477&action=download))
*   Hikvision IP cameras provide a `multipart/mixed` stream of events,
    as described
    [here](https://github.com/scottlamb/moonfire-playground/blob/4e6a786286272ee36f449d761740191c6e6a54fc/camera-motion/src/hikvision.rs#L33).
*   wikipedia [mentions](https://en.wikipedia.org/wiki/MIME#Mixed-Replace)
    that IP cameras use this format for MJPEG streams.

There's a big limitation, however, which is that browsers have fairly low
limits on the number of concurrent connections. In Chrome's case, six per
host. For this reason, multipart streams are only suitable in HTTP APIs where
the clients are *not* web browsers.

## What is a multipart stream exactly?

A multipart response might look like this:

```
Content-Type: multipart/x-mixed-replace: boundary=B

--B
Content-Type: text/plain
Content-Length: 3

foo

--B
Content-Type: text/plain
Content-Length: 3

bar
```

and is typically paired with `Transfer-Encoding: chunked` or `Connection:
close` to allow sending a response whose size is infinite or not known until
the end.

I can't find a good specification. [This WHATWG
document](https://html.spec.whatwg.org/multipage/iana.html#multipart/x-mixed-replace)
describes `multipart/x-mixed-replace` loosely. It refers to [RFC
2046](https://tools.ietf.org/html/rfc2046) which defines multipart encodings
originally used for rich emails. I don't think these HTTP multipart streams
quite follow that RFC. My library currently requires:

*   Any content type—the caller should validate this to taste and pass the
    boundary parameter to the `multipart-stream` library.
*   No preamble. That is, no arbitrary bytes to discard before the first
    part's boundary. (Newlines are okay.)
*   Zero or more newlines (to be precise: `\r\n` sequences) between each part
    and the next part's boundary.
*   A `Content-Length` line for each part. This is a much cleaner approach
    than producers attempting to choose a boundary that doesn't appear in any
    part and consumers having to search through the part body.
*   No extra `--` suffix on the final part's boundary. I've never seen one.

Please open a github issue if you encounter a multipart stream which doesn't
match these requirements.

## What does this library do?

It takes a stream of `Bytes` (such as those returned by
[reqwest](https://crates.io/crates/reqwest) or
[hyper](https://crates.io/crates/hyper)) and returns a stream of
`multipart_stream::Part`s, or vice versa. (For client or server use cases,
respectively.) Each `Part` packages together headers and a body.

## Author

Scott Lamb &lt;slamb@slamb.org>

## License

SPDX-License-Identifier: [MIT](https://spdx.org/licenses/MIT.html) OR [Apache-2.0](https://spdx.org/licenses/Apache-2.0.html)

See [LICENSE-MIT.txt](LICENSE-MIT.txt) or [LICENSE-APACHE](LICENSE-APACHE.txt), respectively.
