//! Heph, derived from [Hephaestus], is the Greek god of blacksmiths,
//! metalworking, carpenters, craftsmen, artisans, sculptors, metallurgy, fire,
//! and volcanoes. Well this crate has very little to do with Greek gods, but I
//! needed a name.
//!
//! [Hephaestus]: https://en.wikipedia.org/wiki/Hephaestus
//!
//! ## About
//!
//! Heph is an [actor] framework based on asynchronous functions. Such an
//! asynchronous function looks like this:
//!
//! ```
//! # use heph::actor;
//! # use heph_rt::ThreadLocal;
//! #
//! async fn actor(mut ctx: actor::Context<String, ThreadLocal>) {
//!     // Receive a message.
//!     if let Ok(msg) = ctx.receive_next().await {
//!         // Print the message.
//!         println!("got a message: {msg}");
//!     }
//! }
//! #
//! # drop(actor); // Silence dead code warnings.
//! ```
//!
//! Heph uses an event-driven, non-blocking I/O, share nothing design. But what
//! do all those buzzwords actually mean?
//!
//!  - *Event-driven*: Heph does nothing by itself, it must first get an event
//!    before it starts doing anything. For example when using a `TcpListener`
//!    it waits on a notification from the OS saying the `TcpListener` is ready
//!    before trying to accept connections.
//!  - *Non-blocking I/O*: normal I/O operations need to wait (block) until the
//!    operation can complete. Using non-blocking, or asynchronous, I/O means
//!    that rather then waiting for the operation to complete we'll do some
//!    other, more useful, work and try the operation later.
//!  - *Share nothing*: a lot of application share data across multiple threads.
//!    To do this safely we need to protect it from data races, via a [`Mutex`]
//!    or by using [atomic] operations. Heph is designed to not share any data.
//!    Each actor is responsible for its own memory and cannot access memory
//!    owned by other actors. Instead communication is done via sending
//!    messages, see the [actor model].
//!
//! [actor]: https://en.wikipedia.org/wiki/Actor_model
//! [`Mutex`]: std::sync::Mutex
//! [atomic]: std::sync::atomic
//! [actor model]: https://en.wikipedia.org/wiki/Actor_model
//!
//! ## Getting started
//!
//! There are two ways to get starting with Heph. If you like to see examples,
//! take a look at the [examples] in the examples directory of the source code.
//! If you like to learn more about some of the core concepts of Heph start with
//! the [Quick Start] guide.
//!
//! [examples]: https://github.com/Thomasdezeeuw/heph/blob/master/examples/README.md
//! [Quick Start]: crate::quick_start
//!
//! ## Features
//!
//! This crate has one optional: `test`. The `test` feature will enable the
//! `test` module which adds testing facilities.

#![feature(const_option, doc_auto_cfg, doc_cfg_hide, never_type)]
#![warn(
    anonymous_parameters,
    bare_trait_objects,
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    trivial_numeric_casts,
    unused_extern_crates,
    unused_import_braces,
    unused_qualifications,
    unused_results,
    variant_size_differences
)]
// Disallow warnings when running tests.
#![cfg_attr(test, deny(warnings))]
// Disallow warnings in examples, we want to set a good example after all.
#![doc(test(attr(deny(warnings))))]
// The `cfg(any(test, feature = "test"))` attribute creates a doc element
// staying that it's only supporting "using test or test", that is a bit
// confusing. So we hide those parts and instead manually replace all of them
// with: `doc(cfg(feature = "test"))`. That will stay it's only supported using
// the test feature.
#![doc(cfg_hide(any(test, feature = "test")))]

pub mod actor;
pub mod actor_ref;
pub mod messages;
pub mod quick_start;
pub mod supervisor;
#[cfg(any(test, feature = "test"))]
pub mod test;

#[doc(no_inline)]
pub use actor::{Actor, NewActor};
#[doc(no_inline)]
pub use actor_ref::ActorRef;
#[doc(no_inline)]
pub use supervisor::{Supervisor, SupervisorStrategy};
