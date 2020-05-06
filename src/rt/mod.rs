//! Module with Heph's runtime and related types.
//!
//! The module has two main types and a submodule:
//!
//! - [`Runtime`] is Heph's runtime, used to run all actors.
//! - [`RuntimeRef`] is a reference to a running runtime, used for example to
//!   spawn new actors.
//!
//! See [`Runtime`] for documentation on how to run an actor. For more examples
//! see the examples directory in the source code.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;
use std::{fmt, io, task};

use log::{debug, trace};
use mio::{event, Interest, Poll, Token};

use crate::actor::sync::SyncActor;
use crate::actor::{self, context, NewActor};
use crate::actor_ref::ActorRef;
use crate::inbox::Inbox;
use crate::supervisor::{Supervisor, SyncSupervisor};

mod coordinator;
mod error;
mod hack;
mod process;
mod signal;
mod sync_worker;
mod timers;

// Modules that are public (in the crate) when testing and private otherwise.
macro_rules! cfg_pub_test {
    ($( $mod_name: ident ),+) => {
        $(
            #[cfg(not(test))]
            mod $mod_name;
            #[cfg(test)]
            pub(crate) mod $mod_name;
        )+
    };
}

// Modules used in testing.
cfg_pub_test!(channel, scheduler, waker, worker);

pub(crate) use process::ProcessId;
pub(crate) use waker::Waker;

pub mod options;

pub use error::RuntimeError;
pub use options::ActorOptions;
pub use signal::Signal;

use coordinator::Coordinator;
use hack::SetupFn;
use scheduler::{LocalScheduler, SchedulerRef};
use sync_worker::SyncWorker;
use timers::Timers;
use waker::{WakerId, MAX_THREADS};
use worker::Worker;

const SYNC_WORKER_ID_START: usize = MAX_THREADS + 1;

/// The runtime that runs all actors.
///
/// This type implements a builder pattern to build and start a runtime. It has
/// a single generic parameter `S` which is the type of the setup function. The
/// setup function is optional, which is represented by `!` (the never type).
///
/// The runtime will start workers threads that will run all actors, these
/// threads will run until all actors have returned.
///
/// ## Usage
///
/// Building an runtime starts with calling [`new`], this will create a new
/// runtime without a setup function, using a single worker thread.
///
/// An optional setup function can be added by calling [`with_setup`]. This
/// setup function will be called on each worker thread the runtime starts, and
/// thus has to be [`Clone`] and [`Send`]. This will change the `S` parameter
/// from `!` to an actual type (that of the provided setup function). Only a
/// single setup function can be used per runtime.
///
/// Now that the generic parameter is optionally defined we also have some more
/// configuration options. One such option is the number of threads the runtime
/// uses, this can configured with the [`num_threads`] method, or
/// [`use_all_cores`].
///
/// Finally after setting all the configuration options the runtime can be
/// [`start`]ed. This will spawn a number of worker threads to actually run the
/// actors.
///
/// [`new`]: Runtime::new
/// [`with_setup`]: Runtime::with_setup
/// [`num_threads`]: Runtime::num_threads
/// [`use_all_cores`]: Runtime::use_all_cores
/// [`start`]: Runtime::start
///
/// ## Examples
///
/// This simple example shows how to run an `Runtime` and add how start a
/// single actor on each thread. This should print "Hello World" twice (once on
/// each worker thread started).
///
/// ```
/// #![feature(never_type)]
///
/// use heph::supervisor::NoSupervisor;
/// use heph::{actor, RuntimeError, ActorOptions, Runtime, RuntimeRef};
///
/// fn main() -> Result<(), RuntimeError> {
///     // Build a new `Runtime`.
///     Runtime::new()
///         // Start two worker threads.
///         .num_threads(2)
///         // On each worker thread run our setup function.
///         .with_setup(setup)
///         // And start the runtime.
///         .start()
/// }
///
/// // This setup function will on run on each created thread. In the case of
/// // this example we create two threads (see `main`).
/// fn setup(mut runtime_ref: RuntimeRef) -> Result<(), !> {
///     // Asynchronous function don't yet implement the required `NewActor`
///     // trait, but function pointers do so we cast our type to a function
///     // pointer.
///     let actor = actor as fn(_, _) -> _;
///     // Each actors needs to be supervised, but since our actor doesn't
///     // return an error (the error type is `!`) we can get away with our
///     // `NoSupervisor` which doesn't do anything (as it can never be called).
///     let supervisor = NoSupervisor;
///     // All actors start with one or more arguments, in our case it's message
///     // used to greet people with. See `actor` below.
///     let arg = "Hello";
///     // Spawn the actor to run on our actor runtime. We get an actor reference
///     // back which can be used to send the actor messages, see below.
///     let mut actor_ref = runtime_ref.spawn_local(supervisor, actor, arg, ActorOptions::default());
///
///     // Send a message to the actor we just spawned.
///     actor_ref <<= "World";
///
///     Ok(())
/// }
///
/// /// Our actor that greets people.
/// async fn actor(mut ctx: actor::Context<&'static str>, msg: &'static str) -> Result<(), !> {
///     // `msg` is the argument passed to `spawn` in the `setup` function
///     // above, in this example it was "Hello".
///
///     // Using the context we can receive messages send to this actor, so here
///     // we'll receive the "World" message we send in the `setup` function.
///     let name = ctx.receive_next().await;
///     // This should print "Hello world"!
///     println!("{} {}", msg, name);
///     Ok(())
/// }
/// ```
pub struct Runtime<S = !> {
    /// Number of worker threads to create.
    threads: usize,
    /// Synchronous actor thread handles.
    sync_actors: Vec<SyncWorker>,
    /// Optional setup function.
    setup: Option<S>,
}

impl Runtime {
    /// Create a `Runtime` with the default configuration.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Runtime<!> {
        Runtime {
            threads: 1,
            sync_actors: Vec::new(),
            setup: None,
        }
    }

    /// Add a setup function.
    ///
    /// This function will be run on each worker thread the runtime creates.
    /// Only a single setup function can be added to the runtime.
    pub fn with_setup<F, E>(self, setup: F) -> Runtime<F>
    where
        F: FnOnce(RuntimeRef) -> Result<(), E> + Send + Clone + 'static,
        E: Send,
    {
        Runtime {
            threads: self.threads,
            sync_actors: self.sync_actors,
            setup: Some(setup),
        }
    }
}

impl<S> Runtime<S> {
    /// Set the number of worker threads to use, defaults to one.
    ///
    /// Most applications would want to use [`Runtime::use_all_cores`] which
    /// sets the number of threads equal to the number of CPU cores.
    pub fn num_threads(mut self, n: usize) -> Self {
        if n > MAX_THREADS {
            panic!(
                "Can't create {} worker threads, {} is the maximum",
                n, MAX_THREADS
            );
        }
        self.threads = n;
        self
    }

    /// Set the number of worker threads equal to the number of CPU cores.
    ///
    /// See [`Runtime::num_threads`].
    pub fn use_all_cores(self) -> Self {
        self.num_threads(num_cpus::get())
    }

    /// Spawn an synchronous actor that runs on its own thread.
    ///
    /// For more information and examples of synchronous actors see the
    /// [`actor::sync`] module.
    ///
    /// [`actor::sync`]: crate::actor::sync
    pub fn spawn_sync_actor<Sv, A, E, Arg, M>(
        &mut self,
        supervisor: Sv,
        actor: A,
        arg: Arg,
    ) -> Result<ActorRef<M>, RuntimeError>
    where
        Sv: SyncSupervisor<A> + Send + 'static,
        A: SyncActor<Message = M, Argument = Arg, Error = E> + Send + 'static,
        Arg: Send + 'static,
        M: Send + 'static,
    {
        let id = SYNC_WORKER_ID_START + self.sync_actors.len();
        SyncWorker::start(id, supervisor, actor, arg)
            .map(|(worker, actor_ref)| {
                self.sync_actors.push(worker);
                actor_ref
            })
            .map_err(RuntimeError::start_thread)
    }
}

impl<S> Runtime<S>
where
    S: SetupFn,
{
    /// Run the runtime.
    ///
    /// This will spawn a number of worker threads (see
    /// [`Runtime::num_threads`]) to run. After spawning all worker threads it
    /// will wait until all worker threads have returned, which happens when all
    /// actors have returned.
    ///
    /// In addition to waiting for all worker threads it will also watch for
    /// all process signals in [`Signal`] and relay them to actors that want to
    /// handle them, see the [`Signal`] type for more information.
    pub fn start(self) -> Result<(), RuntimeError<S::Error>> {
        debug!(
            "starting Heph runtime: worker_threads={}, sync_actors={}",
            self.threads,
            self.sync_actors.len()
        );

        let coordinator = Coordinator::init().map_err(RuntimeError::coordinator)?;
        let scheduler_ref = coordinator.scheduler_ref();
        let waker_id = coordinator.waker_id();

        // Start our worker threads.
        let handles = (0..self.threads)
            .map(|id| {
                let setup = self.setup.clone();
                Worker::start(id, setup, scheduler_ref.clone(), waker_id)
            })
            .collect::<io::Result<Vec<Worker<S::Error>>>>()
            .map_err(RuntimeError::start_thread)?;

        coordinator.run(handles, self.sync_actors)
    }
}

impl<S> fmt::Debug for Runtime<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let setup = if self.setup.is_some() {
            &"Some"
        } else {
            &"None"
        };
        f.debug_struct("Runtime")
            .field("threads", &self.threads)
            .field("sync_actors (length)", &self.sync_actors.len())
            .field("setup", setup)
            .finish()
    }
}

/// A reference to an [`Runtime`].
///
/// This reference refers to the thread-local runtime, and thus can't be shared
/// across thread bounds. To share this reference (within the same thread) it
/// can be cloned.
///
/// This is passed to the [setup function] when starting the runtime and can be
/// accessed inside an actor by using the [`actor::Context::runtime`] method.
///
/// [setup function]: Runtime::with_setup
#[derive(Clone, Debug)]
pub struct RuntimeRef {
    /// A shared reference to the runtime's internals.
    internal: Rc<RuntimeInternal>,
}

impl RuntimeRef {
    /// Attempts to spawn a local actor.
    ///
    /// Arguments:
    /// * `supervisor`: all actors need supervision, the `supervisor` is the
    ///   supervisor for this actor, see the [`Supervisor`] trait for more
    ///   information.
    /// * `new_actor`: the [`NewActor`] implementation that defines how to start
    ///   the actor.
    /// * `arg`: the argument passed when starting the actor, and
    /// * `options`: the actor options used to spawn the new actors.
    ///
    /// See [`Runtime`] for an example on spawning actors.
    ///
    /// # Notes
    ///
    /// This actor will remain on the thread on which it is started, this both a
    /// good and a bad thing. The good side of it is that it can be `!Send` and
    /// `!Sync`, meaning that it can use cheaper operations is some cases. The
    /// downside is that if a single actor blocks it will block *all* actors on
    /// this thread. Something that some framework work around with actor/tasks
    /// that transparently move between threads and hide blocking/bad actors.
    ///
    /// When using a [`NewActor`] implementation that never returns an error,
    /// such as the implementation provided by async function, it's easier to
    /// use the [`spawn_local`] method.
    ///
    /// [`spawn_local`]: RuntimeRef::spawn_local
    pub fn try_spawn_local<S, NA>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg: NA::Argument,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, NA::Error>
    where
        S: Supervisor<NA> + 'static,
        NA: NewActor<Context = context::ThreadLocal> + 'static,
        NA::Actor: 'static,
    {
        self.try_spawn_local_setup(supervisor, new_actor, |_, _| Ok(arg), options)
            .map_err(|err| match err {
                AddActorError::NewActor(err) => err,
                AddActorError::<_, !>::ArgFn(_) => unreachable!(),
            })
    }

    /// Spawn an actor.
    ///
    /// This is a convenience method for `NewActor` implementations that never
    /// return an error, such as asynchronous functions.
    ///
    /// See [`RuntimeRef::try_spawn_local`] for more information.
    pub fn spawn_local<S, NA>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg: NA::Argument,
        options: ActorOptions,
    ) -> ActorRef<NA::Message>
    where
        S: Supervisor<NA> + 'static,
        NA: NewActor<Error = !, Context = context::ThreadLocal> + 'static,
        NA::Actor: 'static,
    {
        self.try_spawn_local_setup(supervisor, new_actor, |_, _| Ok(arg), options)
            .unwrap_or_else(|_: AddActorError<!, !>| unreachable!())
    }

    /// Spawn an actor that needs to be initialised.
    ///
    /// Just like `try_spawn_local` this requires a `supervisor`, `new_actor`
    /// and `options`. The only difference being rather then passing an argument
    /// directly this function requires a function to create the argument, which
    /// allows the caller to do any required setup work.
    #[allow(clippy::type_complexity)] // Not part of the public API, so it's OK.
    pub(crate) fn try_spawn_local_setup<S, NA, ArgFn, ArgFnE>(
        &mut self,
        supervisor: S,
        mut new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, ArgFnE>>
    where
        S: Supervisor<NA> + 'static,
        NA: NewActor<Context = context::ThreadLocal> + 'static,
        ArgFn: FnOnce(ProcessId, &mut RuntimeRef) -> Result<NA::Argument, ArgFnE>,
        NA::Actor: 'static,
    {
        // Setup adding a new process to the scheduler.
        let mut scheduler = self.internal.scheduler.borrow_mut();
        let actor_entry = scheduler.add_actor();
        let pid = actor_entry.pid();
        debug!("spawning actor: pid={}", pid);

        // Create our actor argument, running any setup required by the caller.
        let mut runtime_ref = self.clone();
        let arg = arg_fn(pid, &mut runtime_ref).map_err(AddActorError::ArgFn)?;

        let waker = self.new_waker(pid);
        if options.is_ready() {
            waker.wake();
        }

        // Create our actor context and our actor with it.
        let (inbox, inbox_ref) = Inbox::new(waker);
        let actor_ref = ActorRef::from_inbox(inbox_ref.clone());
        let ctx = actor::Context::new(pid, inbox.ctx_inbox(), inbox_ref.clone(), runtime_ref);
        let actor = new_actor.new(ctx, arg).map_err(AddActorError::NewActor)?;

        // Add the actor to the scheduler.
        actor_entry.add(
            options.priority(),
            supervisor,
            new_actor,
            actor,
            inbox,
            inbox_ref,
        );

        Ok(actor_ref)
    }

    /// Attempts to spawn a thread-safe actor.
    ///
    /// Actor spawned using this method can be run on any of the worker threads.
    /// This has the upside of less starvation at the cost of decreased
    /// performance as the implementation backing thread-local actors is faster.
    ///
    /// The arguments are the same as [`try_spawn_local`], but require the
    /// `supervisor` (`S`), actor (`NA::Actor`) and `new_actor` (`NA`) to be
    /// `Send`.
    ///
    /// # Notes
    ///
    /// When using a [`NewActor`] implementation that never returns an error,
    /// such as the implementation provided by async function, it's easier to
    /// use the [`spawn`] method.
    ///
    /// [`try_spawn_local`]: RuntimeRef::try_spawn_local
    /// [`spawn`]: RuntimeRef::spawn
    pub fn try_spawn<S, NA>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg: NA::Argument,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, NA::Error>
    where
        S: Supervisor<NA> + Send + 'static,
        NA: NewActor<Context = context::ThreadSafe> + Send + 'static,
        NA::Actor: Send + 'static,
    {
        self.try_spawn_setup(supervisor, new_actor, |_| Ok(arg), options)
            .map_err(|err| match err {
                AddActorError::NewActor(err) => err,
                AddActorError::<_, !>::ArgFn(_) => unreachable!(),
            })
    }

    /// Spawn an thread-safe actor.
    ///
    /// This is a convenience method for `NewActor` implementations that never
    /// return an error, such as asynchronous functions.
    ///
    /// See [`RuntimeRef::try_spawn`] for more information.
    pub fn spawn<S, NA>(
        &mut self,
        supervisor: S,
        new_actor: NA,
        arg: NA::Argument,
        options: ActorOptions,
    ) -> ActorRef<NA::Message>
    where
        S: Supervisor<NA> + Send + 'static,
        NA: NewActor<Error = !, Context = context::ThreadSafe> + Send + 'static,
        NA::Actor: Send + 'static,
    {
        self.try_spawn_setup(supervisor, new_actor, |_| Ok(arg), options)
            .unwrap_or_else(|_: AddActorError<!, !>| unreachable!())
    }

    /// Spawn an thread-safe actor that needs to be initialised.
    ///
    /// Just like `try_spawn` this requires a `supervisor`, `new_actor` and
    /// `options`. The only difference being rather then passing an argument
    /// directly this function requires a function to create the argument, which
    /// allows the caller to do any required setup work.
    #[allow(clippy::type_complexity)] // Not part of the public API, so it's OK.
    pub(crate) fn try_spawn_setup<S, NA, ArgFn, ArgFnE>(
        &mut self,
        supervisor: S,
        mut new_actor: NA,
        arg_fn: ArgFn,
        options: ActorOptions,
    ) -> Result<ActorRef<NA::Message>, AddActorError<NA::Error, ArgFnE>>
    where
        S: Supervisor<NA> + Send + 'static,
        NA: NewActor<Context = context::ThreadSafe> + Send + 'static,
        ArgFn: FnOnce(ProcessId) -> Result<NA::Argument, ArgFnE>,
        NA::Actor: Send + 'static,
    {
        let RuntimeInternal {
            coordinator_id,
            scheduler_ref,
            ..
        } = &*self.internal;

        // Setup adding a new process to the scheduler.
        let mut scheduler_ref = scheduler_ref.borrow_mut();
        let actor_entry = scheduler_ref.add_actor();
        let pid = actor_entry.pid();
        debug!("spawning actor: pid={}", pid);

        // Create our actor argument, running any setup required by the caller.
        let arg = arg_fn(pid).map_err(AddActorError::ArgFn)?;

        let waker = Waker::new(*coordinator_id, pid);
        if options.is_ready() {
            waker.wake()
        }

        // Create our actor context and our actor with it.
        let (inbox, inbox_ref) = Inbox::new(waker);
        let actor_ref = ActorRef::from_inbox(inbox_ref.clone());
        let ctx = actor::Context::new(pid, inbox.ctx_inbox(), inbox_ref.clone(), self.clone());
        let actor = new_actor.new(ctx, arg).map_err(AddActorError::NewActor)?;

        // Add the actor to the scheduler.
        actor_entry.add(
            options.priority(),
            supervisor,
            new_actor,
            actor,
            inbox,
            inbox_ref,
        );

        Ok(actor_ref)
    }

    /// Receive [process signals] as messages.
    ///
    /// This adds the `actor_ref` to the list of actor references that will
    /// receive a process signal.
    ///
    /// [process signals]: Signal
    pub fn receive_signals(&mut self, actor_ref: ActorRef<Signal>) {
        self.internal.signal_receivers.borrow_mut().push(actor_ref)
    }

    /// Register an `event::Source`, see [`mio::Registry::register`].
    pub(crate) fn register<S>(
        &mut self,
        source: &mut S,
        token: Token,
        interest: Interest,
    ) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.internal
            .poll
            .borrow()
            .registry()
            .register(source, token, interest)
    }

    /// Reregister an `event::Source`, see [`mio::Registry::reregister`].
    pub(crate) fn reregister<S>(
        &mut self,
        source: &mut S,
        token: Token,
        interest: Interest,
    ) -> io::Result<()>
    where
        S: event::Source + ?Sized,
    {
        self.internal
            .poll
            .borrow()
            .registry()
            .reregister(source, token, interest)
    }

    /// Create a new `Waker` implementation.
    pub(crate) fn new_waker(&self, pid: ProcessId) -> Waker {
        Waker::new(self.internal.waker_id, pid)
    }

    /// Get a clone of the sending end of the notification channel.
    ///
    /// # Notes
    ///
    /// Prefer `new_waker` if possible, only use `task::Waker` for `Future`s.
    pub(crate) fn new_task_waker(&self, pid: ProcessId) -> task::Waker {
        waker::new(self.internal.waker_id, pid)
    }

    /// Add a deadline to the event sources.
    ///
    /// This is used in the `timer` crate.
    pub(crate) fn add_deadline(&mut self, pid: ProcessId, deadline: Instant) {
        trace!("adding deadline: pid={}, deadline={:?}", pid, deadline);
        self.internal
            .timers
            .borrow_mut()
            .add_deadline(pid, deadline);
    }
}

/// Internal error returned by spawning a actor.
#[derive(Debug)]
pub(crate) enum AddActorError<NewActorE, ArgFnE> {
    /// Calling `NewActor::new` actor resulted in an error.
    NewActor(NewActorE),
    /// Calling the argument function resulted in an error.
    ArgFn(ArgFnE),
}

/// Internals of the runtime, to which `RuntimeRef`s have a reference.
#[derive(Debug)]
struct RuntimeInternal {
    /// Waker id used to create a `Waker`.
    waker_id: WakerId,
    /// A reference to the scheduler to add new processes to.
    scheduler: RefCell<LocalScheduler>,
    /// Working stealing reference to the shared scheduler.
    scheduler_ref: RefCell<SchedulerRef>,
    /// Waker id for the `Coordinator`.
    coordinator_id: WakerId,
    /// System poll, used for event notifications to support non-blocking I/O.
    poll: RefCell<Poll>,
    /// Timers, deadlines and timeouts.
    timers: RefCell<Timers>,
    /// Actor references to relay received `Signal`s to.
    signal_receivers: RefCell<Vec<ActorRef<Signal>>>,
}
