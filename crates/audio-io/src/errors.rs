//! Errors reported by a cpal stream, passed from its error callback to the control thread.
//!
//! The callback side, [`ErrorSink`], is real-time safe: it bumps a counter and moves the
//! error cpal handed it into a bounded lock-free queue. It never allocates, locks, blocks
//! or prints. The control thread reads the count and the messages from [`StreamErrors`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

/// How many error messages wait for the control thread before new ones are dropped (they
/// are still counted).
pub const ERROR_QUEUE_CAPACITY: usize = 16;

/// A connected pair: the sink goes into the stream's error callback, the reader stays on
/// the control thread.
pub fn stream_errors() -> (ErrorSink, StreamErrors) {
    let count = Arc::new(AtomicU64::new(0));
    let (producer, consumer) = RingBuffer::new(ERROR_QUEUE_CAPACITY);
    (
        ErrorSink {
            count: Arc::clone(&count),
            queue: producer,
        },
        StreamErrors {
            count,
            queue: consumer,
        },
    )
}

/// The error callback's end.
pub struct ErrorSink {
    count: Arc<AtomicU64>,
    queue: Producer<cpal::Error>,
}

impl ErrorSink {
    /// Counts `error` and moves it into the queue. When the queue is full the error is
    /// dropped (freeing whatever cpal allocated for it); it is still counted.
    pub fn report(&mut self, error: cpal::Error) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let _ = self.queue.push(error);
    }
}

/// The control thread's end.
pub struct StreamErrors {
    count: Arc<AtomicU64>,
    queue: Consumer<cpal::Error>,
}

impl StreamErrors {
    /// How many errors were reported so far, including those whose message was dropped.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// The messages of the errors reported since the last call, oldest first.
    pub fn take(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.queue.pop().ok())
            .map(|error| error.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use cpal::{Error, ErrorKind};

    use super::*;

    #[test]
    fn messages_reach_the_reader_in_order() {
        let (mut sink, mut errors) = stream_errors();
        assert!(errors.take().is_empty());
        sink.report(Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "unplugged",
        ));
        sink.report(Error::new(ErrorKind::DeviceNotAvailable));
        assert_eq!(errors.count(), 2);
        let messages = errors.take();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], "unplugged");
        assert!(messages[1].contains("not available"), "{}", messages[1]);
        assert!(errors.take().is_empty(), "taken once");
        assert_eq!(errors.count(), 2, "the count is kept");
    }

    #[test]
    fn a_full_queue_drops_messages_but_counts_them() {
        let (mut sink, mut errors) = stream_errors();
        for _ in 0..ERROR_QUEUE_CAPACITY + 5 {
            sink.report(Error::new(ErrorKind::DeviceBusy));
        }
        assert_eq!(errors.count(), (ERROR_QUEUE_CAPACITY + 5) as u64);
        assert_eq!(errors.take().len(), ERROR_QUEUE_CAPACITY);
    }
}
