#![no_main]
use futures::{task::Poll, Stream, StreamExt};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Look for panics or obvious misbehavior in the parsing implementation.
    // Divide data into parts (by a u8 length prefix) and feed them over a channel, polling the
    // stream after each until it's pending. When there are no more full parts, just break.
    let (mut tx, rx) = futures::channel::mpsc::channel(0);
    let stream = multipart_stream::parser::parse(rx.map(Ok::<_, std::convert::Infallible>), "foo");
    futures::pin_mut!(stream);
    let waker = futures::task::noop_waker();
    let mut cx = futures::task::Context::from_waker(&waker);
    let mut data = data;
    while data.len() > 1 {
        let next_part_size = usize::from(data[0]);
        if data.len() < 1 + next_part_size {
            break;
        }
        let next_part = bytes::Bytes::copy_from_slice(&data[1..1 + next_part_size]);
        tx.try_send(next_part).expect("previous stream poll should have emptied the channel");
        while let Poll::Ready(r) = stream.as_mut().poll_next(&mut cx) {
            match r {
                Some(Err(_)) => return,
                None => panic!("tx hasn't ended => stream shouldn't end"),
                _ => {}
            }
        }
        data = &data[1 + next_part_size..];
    }

    // Handle end of stream.
    drop(tx);
    match stream.poll_next(&mut cx) {
        Poll::Pending => panic!("tx has ended => stream shouldn't be pending"),
        Poll::Ready(Some(Err(_))) => return, // an error about an unfinished part is normal
        Poll::Ready(Some(Ok(_))) => panic!("tx has ended => stream shouldn't have more data"),
        Poll::Ready(None) => {}
    }
});
