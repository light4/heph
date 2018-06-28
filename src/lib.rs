//! TODO: docs.

#![feature(arbitrary_self_types,
           const_fn,
           futures_api,
           non_exhaustive,
           read_initializer,
           never_type,
)]

#![warn(missing_debug_implementations,
        missing_docs,
        trivial_casts,
        trivial_numeric_casts,
        unused_import_braces,
        unused_qualifications,
        unused_results,
)]

#[macro_use]
extern crate log;
extern crate mio_st;
extern crate num_cpus;
extern crate slab;

pub mod actor;
pub mod initiator;
pub mod net;
pub mod supervisor;
pub mod system;

mod process;
mod scheduler;

/// The actor prelude. All useful traits and types in single module.
///
/// ```
/// use actor::prelude::*;
/// ```
pub mod prelude {
    pub use actor::{Actor, NewActor};
    pub use supervisor::{Supervisor, RestartStrategy};
    pub use system::{ActorSystem, ActorSystemBuilder, ActorOptions, ActorRef};
}
