//! Module with time related utilities.
//!
//! This module provides three types.
//!
//! - [`Timer`](Timer) is a stand-alone future that returns
//!   [`DeadlinePassed`](DeadlinePassed) once the deadline has passed.
//! - [`Deadline`](Deadline) wraps another `Future` and checks the
//!   deadline each time it's polled, it returns `Err(DeadlinePassed)` once the
//!   deadline has passed.
//! - [`Interval`](Interval) implements [`Stream`] which yields an item
//!   after the deadline has passed each interval.
//!
//! [`Stream`]: futures_core::stream::Stream

// TODO: support thread-safe actors.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};
use std::time::{Duration, Instant};

use futures_core::stream::{FusedStream, Stream};

use crate::actor::context::ThreadLocal;
use crate::rt::{ProcessId, RuntimeAccess};
use crate::{actor, RuntimeRef};

/// Type returned when the deadline has passed.
///
/// Can be converted into [`io::ErrorKind::TimedOut`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DeadlinePassed;

impl From<DeadlinePassed> for io::Error {
    fn from(_: DeadlinePassed) -> io::Error {
        io::ErrorKind::TimedOut.into()
    }
}

/// A [`Future`] that represents a timer.
///
/// If this future returns [`Poll::Ready`]`(`[`DeadlinePassed`]`)` it means that
/// the deadline has passed. If it returns [`Poll::Pending`] it's not yet
/// passed.
///
/// # Examples
///
/// ```
/// #![feature(never_type)]
///
/// use std::time::Duration;
/// # use std::time::Instant;
///
/// use heph::actor;
/// # use heph::supervisor::NoSupervisor;
/// # use heph::{rt, ActorOptions, Runtime, RuntimeRef};
/// use heph::timer::Timer;
///
/// # fn main() -> Result<(), rt::Error> {
/// #     Runtime::new()?.with_setup(setup).start()
/// # }
/// #
/// #
/// # fn setup(mut runtime_ref: RuntimeRef) -> Result<(), !> {
/// #   let actor = actor as fn(_) -> _;
/// #   let options = ActorOptions::default().mark_ready();
/// #   runtime_ref.spawn_local(NoSupervisor, actor, (), options);
/// #   Ok(())
/// # }
/// #
/// async fn actor(mut ctx: actor::Context<!>) -> Result<(), !> {
/// #   let start = Instant::now();
///     // Create a timer, this will be ready once the timeout has passed.
///     let timeout = Timer::timeout(&mut ctx, Duration::from_millis(200));
/// #   assert!(timeout.deadline() >= start + Duration::from_millis(200));
///
///     // Wait for the timer to pass.
///     timeout.await;
/// #   assert!(Instant::now() >= start + Duration::from_millis(200));
///     println!("One second has passed!");
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct Timer {
    deadline: Instant,
}

impl Timer {
    /// Create a new `Timer`.
    pub fn new<M, K>(ctx: &mut actor::Context<M, K>, deadline: Instant) -> Timer
    where
        K: RuntimeAccess,
    {
        let pid = ctx.pid();
        ctx.kind().add_deadline(pid, deadline);
        Timer { deadline }
    }

    /// Create a new timer, based on a timeout.
    ///
    /// Same as calling `Timer::new(&mut ctx, Instant::now() + timeout)`.
    pub fn timeout<M, K>(ctx: &mut actor::Context<M, K>, timeout: Duration) -> Timer
    where
        K: RuntimeAccess,
    {
        Timer::new(ctx, Instant::now() + timeout)
    }

    /// Returns the deadline set for this `Timer`.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Returns `true` if the deadline has passed.
    pub fn has_passed(&self) -> bool {
        self.deadline <= Instant::now()
    }

    /// Wrap a future creating a new `Deadline`.
    pub fn wrap<Fut>(self, future: Fut) -> Deadline<Fut> {
        // Already added a deadline so no need to do it again.
        Deadline {
            deadline: self.deadline,
            future,
        }
    }
}

impl Future for Timer {
    type Output = DeadlinePassed;

    fn poll(self: Pin<&mut Self>, _ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        if self.has_passed() {
            Poll::Ready(DeadlinePassed)
        } else {
            Poll::Pending
        }
    }
}

impl<K> actor::Bound<K> for Timer
where
    K: RuntimeAccess,
{
    type Error = !;

    fn bind_to<M>(&mut self, ctx: &mut actor::Context<M, K>) -> Result<(), !> {
        // We don't remove the original deadline and just let it expire, as
        // (currently) removing a deadline is an expensive operation.
        let pid = ctx.pid();
        ctx.kind().add_deadline(pid, self.deadline);
        Ok(())
    }
}

/// A [`Future`] that wraps another future setting a deadline for it.
///
/// When this future is polled it first checks if the deadline has passed, if so
/// it returns [`Poll::Ready`]`(Err(`[`DeadlinePassed`]`))`. Otherwise this will
/// poll the future it wraps.
///
/// # Notes
///
/// This type can also be created using [`Timer::wrap`], this is useful when
/// dealing with lifetime issue, e.g. when calling
/// [`actor::Context::receive_next`] and wrapping that in a `Deadline`.
///
/// # Examples
///
/// Setting a timeout for a future.
///
/// ```
/// #![feature(never_type)]
///
/// use std::io;
/// # use std::future::Future;
/// # use std::pin::Pin;
/// # use std::task::{self, Poll};
/// use std::time::Duration;
/// # use std::time::Instant;
///
/// use heph::actor;
/// use heph::actor::context::ThreadSafe;
/// # use heph::supervisor::NoSupervisor;
/// # use heph::{rt, ActorOptions, Runtime};
/// use heph::timer::Deadline;
///
/// # fn main() -> Result<(), rt::Error> {
/// #     let actor = actor as fn(_) -> _;
/// #     let options = ActorOptions::default().mark_ready();
/// #     let mut rt = Runtime::new()?;
/// #     let _ = rt.spawn(NoSupervisor, actor, (), options);
/// #     rt.start()
/// # }
/// #
/// # struct IoFuture;
/// #
/// # impl Future for IoFuture {
/// #     type Output = io::Result<()>;
/// #     fn poll(self: Pin<&mut Self>, _ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
/// #         Poll::Pending
/// #     }
/// # }
/// #
/// async fn actor(mut ctx: actor::Context<String, ThreadSafe>) -> Result<(), !> {
///     // `OtherFuture` is a type that implements `Future`.
///     let future = IoFuture;
///     // Create our deadline.
/// #   let start = Instant::now();
///     let deadline_future = Deadline::timeout(&mut ctx, Duration::from_millis(100), future);
/// #   assert!(deadline_future.deadline() >= start + Duration::from_millis(100));
///
///     // Now we await the results.
///     let result = deadline_future.await;
/// #   assert!(Instant::now() >= start + Duration::from_millis(100));
///     // However the other future is rather slow, so the timeout will pass.
///     assert!(result.is_err());
///     assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct Deadline<Fut> {
    deadline: Instant,
    future: Fut,
}

impl<Fut> Deadline<Fut> {
    /// Create a new `Deadline`.
    pub fn new<M, K>(
        ctx: &mut actor::Context<M, K>,
        deadline: Instant,
        future: Fut,
    ) -> Deadline<Fut>
    where
        K: RuntimeAccess,
    {
        let pid = ctx.pid();
        ctx.kind().add_deadline(pid, deadline);
        Deadline { deadline, future }
    }

    /// Create a new deadline based on a timeout.
    ///
    /// Same as calling `Deadline::new(&mut ctx, Instant::now() + timeout,
    /// future)`.
    pub fn timeout<M, K>(
        ctx: &mut actor::Context<M, K>,
        timeout: Duration,
        future: Fut,
    ) -> Deadline<Fut>
    where
        K: RuntimeAccess,
    {
        Deadline::new(ctx, Instant::now() + timeout, future)
    }

    /// Returns the deadline set for this `Deadline`.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Returns `true` if the deadline has passed.
    pub fn has_passed(&self) -> bool {
        self.deadline <= Instant::now()
    }
}

/* TODO: add this once `specialization` feature is stabilised.
impl<Fut> Future for Deadline<Fut>
where
    Fut: Future,
{
    type Output = Result<Fut::Output, DeadlinePassed>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        if self.has_passed() {
            Poll::Ready(Err(DeadlinePassed))
        } else {
            let future = unsafe {
                // This is safe because we're not moving the future.
                Pin::map_unchecked_mut(self, |this| &mut this.future)
            };
            future.poll(ctx).map(Ok)
        }
    }
}
*/

impl<Fut, T, E> Future for Deadline<Fut>
where
    Fut: Future<Output = Result<T, E>>,
    E: From<DeadlinePassed>,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        if self.has_passed() {
            Poll::Ready(Err(DeadlinePassed.into()))
        } else {
            let future = unsafe {
                // This is safe because we're not moving the future.
                Pin::map_unchecked_mut(self, |this| &mut this.future)
            };
            future.poll(ctx)
        }
    }
}

impl<Fut, K> actor::Bound<K> for Deadline<Fut>
where
    K: RuntimeAccess,
{
    type Error = !;

    fn bind_to<M>(&mut self, ctx: &mut actor::Context<M, K>) -> Result<(), !> {
        // We don't remove the original deadline and just let it expire, as
        // (currently) removing a deadline is an expensive operation.
        let pid = ctx.pid();
        ctx.kind().add_deadline(pid, self.deadline);
        Ok(())
    }
}

/// A [`Stream`] that yields an item after an interval has passed.
///
/// This stream will never return `None`, it will always set another deadline
/// and yield another item after the deadline has passed.
///
/// [`Stream`]: futures_core::stream::Stream
///
/// # Notes
///
/// The next deadline will always will be set after this returns `Poll::Ready`.
/// This means that if the interval is very short and the stream is not polled
/// often enough it's possible that actual time between yielding two values can
/// become bigger then the specified interval.
///
/// # Examples
///
/// The following example will print hello world (roughly) every second.
///
/// ```
/// #![feature(never_type)]
///
/// use std::time::Duration;
/// # use std::time::Instant;
///
/// use futures_util::future::ready;
/// use futures_util::stream::StreamExt;
///
/// use heph::actor;
/// # use heph::supervisor::NoSupervisor;
/// # use heph::{rt, ActorOptions, Runtime, RuntimeRef};
/// use heph::timer::Interval;
/// #
/// # fn main() -> Result<(), rt::Error> {
/// #     Runtime::new()?.with_setup(setup).start()
/// # }
/// #
/// # fn setup(mut runtime_ref: RuntimeRef) -> Result<(), !> {
/// #   let actor = actor as fn(_) -> _;
/// #   let options = ActorOptions::default().mark_ready();
/// #   runtime_ref.spawn_local(NoSupervisor, actor, (), options);
/// #   Ok(())
/// # }
///
/// async fn actor(mut ctx: actor::Context<!>) -> Result<(), !> {
/// #   let mut start = Instant::now();
///     let interval = Interval::new(&mut ctx, Duration::from_millis(200));
/// #   assert!(interval.next_deadline() >= start + Duration::from_millis(200));
///     interval
/// #       .take(1)
///         .for_each(|_| {
/// #           assert!(Instant::now() >= start + Duration::from_millis(200));
/// #           start = Instant::now();
///             println!("Hello world");
///             ready(())
///         })
///         .await;
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct Interval {
    interval: Duration,
    deadline: Instant,
    pid: ProcessId,
    runtime_ref: RuntimeRef,
}

impl Interval {
    /// Create a new `Interval`.
    pub fn new<M>(ctx: &mut actor::Context<M>, interval: Duration) -> Interval {
        let deadline = Instant::now() + interval;
        let mut runtime_ref = ctx.runtime().clone();
        let pid = ctx.pid();
        runtime_ref.add_deadline(pid, deadline);
        Interval {
            interval,
            deadline,
            pid,
            runtime_ref,
        }
    }

    /// Returns the next deadline for this `Interval`.
    pub fn next_deadline(&self) -> Instant {
        self.deadline
    }
}

impl Stream for Interval {
    type Item = DeadlinePassed;

    fn poll_next(self: Pin<&mut Self>, _ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        if self.deadline <= Instant::now() {
            // Determine the next deadline.
            let next_deadline = Instant::now() + self.interval;
            let this = Pin::get_mut(self);
            this.deadline = next_deadline;
            this.runtime_ref.add_deadline(this.pid, next_deadline);
            Poll::Ready(Some(DeadlinePassed))
        } else {
            Poll::Pending
        }
    }
}

impl FusedStream for Interval {
    fn is_terminated(&self) -> bool {
        false
    }
}

impl actor::Bound<ThreadLocal> for Interval {
    type Error = !;

    fn bind_to<M>(&mut self, ctx: &mut actor::Context<M>) -> Result<(), !> {
        // We don't remove the original deadline and just let it expire, as
        // (currently) removing a deadline is an expensive operation.
        let pid = ctx.pid();
        let mut runtime_ref = ctx.runtime().clone();
        runtime_ref.add_deadline(pid, self.deadline);
        self.pid = pid;
        self.runtime_ref = runtime_ref;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io;
    use std::task::Poll;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use futures_test::future::FutureTestExt;
    use futures_test::task::noop_context;
    use futures_util::future::pending;

    use super::Deadline;

    #[test]
    fn deadline_get_deadline() {
        let deadline = Instant::now();
        let future = Deadline {
            deadline,
            future: (),
        };
        assert_eq!(future.deadline(), deadline);
    }

    #[test]
    fn deadline_has_passed() {
        let start = Instant::now();
        let timeout = Duration::from_millis(200);
        let deadline = start + timeout;
        let future = Deadline {
            deadline,
            future: (),
        };

        assert!(!future.has_passed());
        sleep(timeout);
        assert!(future.has_passed());
        assert_eq!(future.deadline(), deadline);
    }

    #[test]
    fn deadline_future_unmoved() {
        let future = pending().assert_unmoved();
        let timeout = Duration::from_millis(100);
        let deadline = Instant::now() + timeout;
        let mut future = Box::pin(Deadline { deadline, future });

        let mut ctx = noop_context();
        assert!(future.as_mut().poll(&mut ctx).is_pending());
        sleep(timeout);
        match future.as_mut().poll(&mut ctx) {
            Poll::<io::Result<()>>::Ready(Err(err)) if err.kind() == io::ErrorKind::TimedOut => {}
            res => panic!("unexpected result: {:?}", res),
        }
    }
}
