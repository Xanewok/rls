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