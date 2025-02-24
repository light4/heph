//! Module with [`TcpListener`] and related types.

use std::async_iter::AsyncIterator;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{self, Poll};

use heph::actor;
use mio::{net, Interest};

use crate::net::TcpStream;
use crate::{self as rt, Bound};

/// A TCP socket listener.
///
/// A listener can be created using [`TcpListener::bind`]. After it is created
/// there are two ways to accept incoming [`TcpStream`]s:
///
///  * [`accept`] accepts a single connection, or
///  * [`incoming`] which returns stream of incoming connections.
///
/// [`accept`]: TcpListener::accept
/// [`incoming`]: TcpListener::incoming
///
/// # Examples
///
/// Accepting a single [`TcpStream`], using [`TcpListener::accept`].
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// use std::net::SocketAddr;
///
/// use log::error;
///
/// use heph::{actor, SupervisorStrategy};
/// # use heph_rt::net::TcpStream;
/// use heph_rt::net::TcpListener;
/// use heph_rt::spawn::ActorOptions;
/// use heph_rt::{self as rt, Runtime, RuntimeRef, ThreadLocal};
/// use log::info;
///
/// fn main() -> Result<(), rt::Error> {
///     std_logger::init();
///
///     let mut runtime = Runtime::new()?;
///     runtime.run_on_workers(setup)?;
///     runtime.start()
/// }
///
/// fn setup(mut runtime_ref: RuntimeRef) -> Result<(), !> {
///     let address = "127.0.0.1:8000".parse().unwrap();
///
///     runtime_ref.spawn_local(supervisor, actor as fn(_, _) -> _, address, ActorOptions::default());
/// #   runtime_ref.spawn_local(supervisor, client as fn(_, _) -> _, address, ActorOptions::default());
///
///     Ok(())
/// }
/// #
/// # async fn client(mut ctx: actor::Context<!, ThreadLocal>, address: SocketAddr) -> io::Result<()> {
/// #   let mut stream = TcpStream::connect(&mut ctx, address)?.await?;
/// #   let local_address = stream.local_addr()?.to_string();
/// #   let mut buf = Vec::with_capacity(local_address.len() + 1);
/// #   stream.recv_n(&mut buf, local_address.len()).await?;
/// #   assert_eq!(buf, local_address.as_bytes());
/// #   Ok(())
/// # }
///
/// // Simple supervisor that logs the error and stops the actor.
/// fn supervisor<Arg>(err: io::Error) -> SupervisorStrategy<Arg> {
///     error!("Encountered an error: {err}");
///     SupervisorStrategy::Stop
/// }
///
/// async fn actor(mut ctx: actor::Context<!, ThreadLocal>, address: SocketAddr) -> io::Result<()> {
///     // Create a new listener.
///     let mut listener = TcpListener::bind(&mut ctx, address)?;
///
///     // Accept a connection.
///     let (unbound_stream, peer_address) = listener.accept().await?;
///     info!("accepted connection from: {peer_address}");
///
///     // Next we need to bind the stream to this actor.
///     let mut stream = unbound_stream.bind_to(&mut ctx)?;
///
///     // Next we write the IP address to the connection.
///     let ip = peer_address.to_string();
///     stream.send_all(ip.as_bytes()).await
/// }
/// ```
///
/// Accepting multiple [`TcpStream`]s, using [`TcpListener::incoming`].
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// use std::net::SocketAddr;
///
/// use log::{error, info};
///
/// use heph::{actor, SupervisorStrategy};
/// use heph_rt::net::TcpListener;
/// # use heph_rt::net::TcpStream;
/// use heph_rt::spawn::ActorOptions;
/// use heph_rt::{self as rt, Runtime, RuntimeRef, ThreadLocal};
/// use heph_rt::util::next;
///
/// fn main() -> Result<(), rt::Error> {
///     std_logger::init();
///
///     let mut runtime = Runtime::new()?;
///     runtime.run_on_workers(setup)?;
///     runtime.start()
/// }
///
/// fn setup(mut runtime_ref: RuntimeRef) -> Result<(), !> {
///     let address = "127.0.0.1:8000".parse().unwrap();
///
///     runtime_ref.spawn_local(supervisor, actor as fn(_, _) -> _, address, ActorOptions::default());
/// #   runtime_ref.spawn_local(supervisor, client as fn(_, _) -> _, address, ActorOptions::default());
///
///     Ok(())
/// }
/// #
/// # async fn client(mut ctx: actor::Context<!, ThreadLocal>, address: SocketAddr) -> io::Result<()> {
/// #   let mut stream = TcpStream::connect(&mut ctx, address)?.await?;
/// #   let local_address = stream.local_addr()?.to_string();
/// #   let mut buf = Vec::with_capacity(local_address.len() + 1);
/// #   stream.recv_n(&mut buf, local_address.len()).await?;
/// #   assert_eq!(buf, local_address.as_bytes());
/// #   Ok(())
/// # }
///
/// // Simple supervisor that logs the error and stops the actor.
/// fn supervisor<Arg>(err: io::Error) -> SupervisorStrategy<Arg> {
///     error!("Encountered an error: {err}");
///     SupervisorStrategy::Stop
/// }
///
/// async fn actor(mut ctx: actor::Context<!, ThreadLocal>, address: SocketAddr) -> io::Result<()> {
///     // Create a new listener.
///     let mut listener = TcpListener::bind(&mut ctx, address)?;
///     let mut incoming = listener.incoming();
///     loop {
///         let (unbound_stream, peer_address) = match next(&mut incoming).await {
///             Some(Ok((unbound_stream, peer_address))) => (unbound_stream, peer_address),
///             Some(Err(err)) => return Err(err),
///             None => return Ok(()),
///         };
///
///         info!("accepted connection from: {peer_address}");
///         let mut stream = unbound_stream.bind_to(&mut ctx)?;
///
///         // Next we write the IP address to the connection.
///         let ip = peer_address.to_string();
///         stream.send_all(ip.as_bytes()).await?;
/// #       return Ok(());
///     }
/// }
/// ```
#[derive(Debug)]
pub struct TcpListener {
    /// The underlying TCP listener, backed by Mio.
    socket: net::TcpListener,
}

impl TcpListener {
    /// Creates a new `TcpListener` which will be bound to the specified
    /// `address`.
    ///
    /// # Notes
    ///
    /// The listener is also [bound] to the actor that owns the
    /// `actor::Context`, which means the actor will be run every time the
    /// listener has a connection ready to be accepted.
    ///
    /// [bound]: crate::Bound
    pub fn bind<M, RT>(
        ctx: &mut actor::Context<M, RT>,
        address: SocketAddr,
    ) -> io::Result<TcpListener>
    where
        RT: rt::Access,
    {
        let mut socket = net::TcpListener::bind(address)?;
        ctx.runtime().register(&mut socket, Interest::READABLE)?;
        Ok(TcpListener { socket })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&mut self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    pub fn set_ttl(&mut self, ttl: u32) -> io::Result<()> {
        self.socket.set_ttl(ttl)
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    pub fn ttl(&mut self) -> io::Result<u32> {
        self.socket.ttl()
    }

    /// Attempts to accept a new incoming [`TcpStream`].
    ///
    /// If an accepted TCP stream is returned, the remote address of the peer is
    /// returned along with it.
    ///
    /// If no streams are currently queued this will return an error with the
    /// [kind] set to [`ErrorKind::WouldBlock`]. Most users should prefer to use
    /// [`TcpListener::accept`].
    ///
    /// See the [`TcpListener`] documentation for an example.
    ///
    /// [kind]: io::Error::kind
    /// [`ErrorKind::WouldBlock`]: io::ErrorKind::WouldBlock
    pub fn try_accept(&mut self) -> io::Result<(UnboundTcpStream, SocketAddr)> {
        self.socket.accept().map(|(socket, address)| {
            (
                UnboundTcpStream {
                    stream: TcpStream { socket },
                },
                address,
            )
        })
    }

    /// Accepts a new incoming [`TcpStream`].
    ///
    /// If an accepted TCP stream is returned, the remote address of the peer is
    /// returned along with it.
    ///
    /// See the [`TcpListener`] documentation for an example.
    pub fn accept(&mut self) -> Accept<'_> {
        Accept {
            listener: Some(self),
        }
    }

    /// Returns a stream that iterates over the [`TcpStream`]s being received on
    /// this listener.
    ///
    /// See the [`TcpListener`] documentation for an example.
    pub fn incoming(&mut self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    /// Get the value of the `SO_ERROR` option on this socket.
    ///
    /// This will retrieve the stored error in the underlying socket, clearing
    /// the field in the process. This can be useful for checking errors between
    /// calls.
    pub fn take_error(&mut self) -> io::Result<Option<io::Error>> {
        self.socket.take_error()
    }
}

/// An unbound [`TcpStream`].
///
/// The stream first has to be bound to an actor (using [`bind_to`]), before it
/// can be used.
///
/// [`bind_to`]: UnboundTcpStream::bind_to
#[derive(Debug)]
pub struct UnboundTcpStream {
    stream: TcpStream,
}

impl UnboundTcpStream {
    /// Bind this TCP stream to the actor's `ctx`, allowing it to be used.
    pub fn bind_to<M, RT>(mut self, ctx: &mut actor::Context<M, RT>) -> io::Result<TcpStream>
    where
        RT: rt::Access,
    {
        ctx.runtime()
            .register(
                &mut self.stream.socket,
                Interest::READABLE | Interest::WRITABLE,
            )
            .map(|()| self.stream)
    }
}

/// The [`Future`] behind [`TcpListener::accept`].
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Accept<'a> {
    listener: Option<&'a mut TcpListener>,
}

impl<'a> Future for Accept<'a> {
    type Output = io::Result<(UnboundTcpStream, SocketAddr)>;

    fn poll(mut self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.listener {
            Some(ref mut listener) => try_io!(listener.try_accept()).map(|res| {
                // Only remove the listener if we return a stream.
                self.listener = None;
                res
            }),
            None => panic!("polled Accept after it return Poll::Ready"),
        }
    }
}

/// The [`AsyncIterator`] behind [`TcpListener::incoming`].
#[derive(Debug)]
#[must_use = "AsyncIterators do nothing unless polled"]
pub struct Incoming<'a> {
    listener: &'a mut TcpListener,
}

impl<'a> AsyncIterator for Incoming<'a> {
    type Item = io::Result<(UnboundTcpStream, SocketAddr)>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        try_io!(self.listener.try_accept()).map(Some)
    }
}

impl<RT: rt::Access> Bound<RT> for TcpListener {
    type Error = io::Error;

    fn bind_to<M>(&mut self, ctx: &mut actor::Context<M, RT>) -> io::Result<()> {
        ctx.runtime()
            .reregister(&mut self.socket, Interest::READABLE)
    }
}
