//! Transmission Control Protocol (TCP) related types.
//!
//! Three main types are provided:
//!
//!  * [`TcpListener`] listens for incoming connections.
//!  * [`TcpStream`] represents a single TCP connection.
//!  * [`TcpServer`] is an [`Actor`] that listens for incoming connections and
//!    starts a new actor for each.
//!
//! [`Actor`]: heph::actor::Actor

pub mod listener;
pub mod server;
pub mod stream;

#[doc(no_inline)]
pub use listener::TcpListener;
#[doc(no_inline)]
pub use server::TcpServer;
#[doc(no_inline)]
pub use stream::TcpStream;
