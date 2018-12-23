//! Whenever we send a request, we register a callback to be executed when response
//! is resolved; e.g. rls.request_typed(11, requests::Definition, Params..., |response: requests::Result| { do stuff })
//!
//! Main idea is that we keep a core "event loop" - we block on recv I/O and when
//! we finally receive the message we see what events we have currently still queued up;
//! with this, we could have conditional futures, e.g. checking notifications
//!
//! Can we block separately on, let's say, resolving every queued up "future"?
//! 
//! We should have two points of running - test code and I/O "event loop";
//! We need to be able to "queue" up X events and wait until every one of them
//! is resolved.
//! 
//! Event loop:
//! receive message https://docs.rs/mio/0.6.0/mio/struct.Poll.html (not working on windows?)
//! process queued up messages and potentially clean those up
//! return control to test code?
//! 
//! So we would always need a queued up event:
//!   wait_until_done_indexing would register
//!

use serde_json::Value;
use super::fixtures_dir;

enum Error {
	IO(std::io::Error),
	Parse(serde_json::Error)
}

trait ReadWithTimeout {
	type Data;
	type Error;
	fn read_msg(&mut self) -> Result<Option<Self::Data>, Self::Error>;
}

// TODO: Implement that with mio for ChildStdout
impl<R: Read> ReadWithTimeout for BufReader<R> {
	type Data = serde_json::Value;
	type Error = Error;

	fn read_msg(&mut self) -> Result<Option<serde_json::Value>, Error> {
		let mut line = String::with_capacity(1024);
		self.read_line(&mut line).map_err(Error::IO)?;
		
		match line.trim() {
			line if !line.is_empty() => Ok(serde_json::from_str(line).map_err(Error::Parse)?),
			_ => Ok(None),
		}
	}
}

use std::io::{Read, BufRead, BufReader};

type Pred<'a> = Fn(&Value) -> bool + 'a;
type Body<'a> = Fn(&Value) + 'a;

struct Context<'a, R: ReadWithTimeout> {
	expectations: Vec<Option<(Box<Pred<'a>>, Box<Body<'a>>)>>,
	read: R,
}

impl<'a, R: ReadWithTimeout<Data = serde_json::Value, Error = Error>> Context<'a, R> {
	pub fn exec_on_match(&mut self, pred: impl Fn(&Value) -> bool + 'a, body: impl Fn(&Value) +'a) -> &mut Context<'a, R> {
		self.expectations.push(Some((Box::new(pred), Box::new(body))));

		self
	}

	/// Notifies every registered callback about new data and executes
	/// associated logic for every callback if a given predicate is true
	fn call_back(&mut self, val: &Value) {
		eprintln!("call_back: {:#?}", val);

		for exp in self.expectations.iter_mut() {
			let (pred, body) = exp.as_ref().unwrap();

			if pred(val) {
				body(val);
				*exp = None;
			}
		}
		
		self.expectations.retain(|x| x.is_some());
	}

	fn consume(mut self) {
		loop {
			match self.read.read_msg() {
				Ok(msg) => {
					if let Some(ref msg) = msg { 
						self.call_back(msg);
					}

					if self.expectations.is_empty() || msg.is_none() {
						break;
					}
				},
				Err(e) => panic!(e),
			}
		}
	}
}

#[test]
fn poc() {
	use std::io::{BufRead, BufReader};

	let file = std::fs::File::open(fixtures_dir().join("msgs.data")).unwrap();
	let reader = BufReader::new(file);

	let mut cx = Context { expectations: Vec::new(), read: reader };
	cx.exec_on_match(|val| val.get("jsonrpc").is_some(), |val| eprintln!("It seems a message has `jsonrpc` key"));
	cx.consume();
}
