use std::pin::Pin;
use std::task::{Context, Poll};

use crate::json_row_parser::JsonRowParser;
use bytes::Bytes;
use futures::stream::{FusedStream, Stream};
use nu_protocol::ShellError;

type QueryStream = dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send;

pub struct JsonRowStream {
    state: State,
    parser: JsonRowParser,
    stream: Pin<Box<QueryStream>>,
}

#[derive(Debug, Eq, PartialEq)]
enum State {
    Collecting,
    Done,
}

impl JsonRowStream {
    pub fn new<S>(stream: S) -> Self
    where
        // TODO: static lifetime?
        S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    {
        Self {
            state: State::Collecting,
            parser: JsonRowParser::new(2),
            stream: Box::pin(stream),
        }
    }
}

impl FusedStream for JsonRowStream {
    fn is_terminated(&self) -> bool {
        matches!(self.state, State::Done)
    }
}

impl Stream for JsonRowStream {
    type Item = Result<Vec<u8>, ShellError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.state {
                State::Collecting => {
                    match this.parser.parse_next() {
                        // We got a value from the parser, propagate it
                        Ok(Some(value)) => return Poll::Ready(Some(Ok(value))),
                        // The parser didn't return a value, so poll the I/O stream to see if it's done
                        Ok(None) => match Pin::new(&mut this.stream).poll_next(cx) {
                            // The I/O stream isn't done yet, but no data is available
                            Poll::Pending => return Poll::Pending,
                            // A chunk is ready from the I/O stream, push it to the parser
                            Poll::Ready(Some(Ok(chunk))) => {
                                this.parser.push(&chunk[..]);
                                continue;
                            }
                            // The I/O Stream is finished, and the parser returned None, we're done.
                            Poll::Ready(None) => return Poll::Ready(None),
                            // The I/O stream errored, propagate the error
                            Poll::Ready(Some(Err(e))) => {
                                let err = if e.is_timeout() {
                                    format!("{}: request timed out - timeouts can be changed through config file or using 'cb-env timeout'", e)
                                } else {
                                    format!("{}: {:?}", e, e)
                                };

                                this.state = State::Done;
                                return Poll::Ready(Some(Err(ShellError::GenericError {
                                    error: err,
                                    msg: "".to_string(),
                                    span: None,
                                    help: None,
                                    inner: vec![],
                                })));
                            }
                        },
                        Err(e) => {
                            this.state = State::Done;
                            return Poll::Ready(Some(Err(e)));
                        }
                    }
                }
                State::Done => return Poll::Ready(None),
            }
        }
    }
}
