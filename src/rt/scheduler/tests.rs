//! Tests for the local scheduler.

use std::cell::RefCell;
use std::future::{pending, Pending};
use std::mem::{self, forget};
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use crate::actor::{self, context, NewActor};
use crate::rt::process::{Process, ProcessId, ProcessResult};
use crate::rt::scheduler::{local, LocalScheduler, Priority, ProcessData};
use crate::rt::RuntimeRef;
use crate::supervisor::NoSupervisor;
use crate::test::{self, init_local_actor_with_inbox, AssertUnmoved};

fn assert_size<T>(expected: usize) {
    assert_eq!(mem::size_of::<T>(), expected);
}

#[test]
fn size_assertions() {
    assert_size::<local::ProcessData>(40);
}

#[derive(Debug)]
struct NopTestProcess;

impl Process for NopTestProcess {
    fn name(&self) -> &'static str {
        "NopTestProcess"
    }

    fn run(self: Pin<&mut Self>, _: &mut RuntimeRef, _: ProcessId) -> ProcessResult {
        unimplemented!();
    }
}

#[test]
fn has_process() {
    let mut scheduler = LocalScheduler::new();
    assert!(!scheduler.has_process());
    assert!(!scheduler.has_ready_process());

    let process: Pin<Box<ProcessData<dyn Process>>> = Box::pin(ProcessData::new(
        Priority::default(),
        Duration::from_secs(0),
        Box::pin(NopTestProcess),
    ));
    scheduler.add_process(process);
    assert!(scheduler.has_process());
    assert!(!scheduler.has_ready_process());
}
async fn simple_actor(_: actor::Context<!>) -> Result<(), !> {
    Ok(())
}

#[test]
fn add_actor() {
    let mut scheduler = LocalScheduler::new();

    let actor_entry = scheduler.add_actor();
    let new_actor = simple_actor as fn(_) -> _;
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        false,
    );
    assert!(scheduler.has_process());
    assert!(!scheduler.has_ready_process());
}

#[test]
fn mark_ready() {
    let mut scheduler = LocalScheduler::new();

    // Incorrect (outdated) pid should be ok.
    scheduler.mark_ready(ProcessId(1));

    let actor_entry = scheduler.add_actor();
    let pid = actor_entry.pid();
    let new_actor = simple_actor as fn(_) -> _;
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        false,
    );

    scheduler.mark_ready(pid);
    assert!(scheduler.has_process());
    assert!(scheduler.has_ready_process());
}

#[test]
fn next_process() {
    let mut scheduler = LocalScheduler::new();

    let actor_entry = scheduler.add_actor();
    let pid = actor_entry.pid();
    let new_actor = simple_actor as fn(_) -> _;
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        false,
    );
    scheduler.mark_ready(pid);

    if let Some(process) = scheduler.next_process() {
        assert_eq!(process.as_ref().id(), pid);
        assert!(!scheduler.has_process());
        assert!(!scheduler.has_ready_process());
    } else {
        panic!("expected a process");
    }
}

#[test]
fn next_process_order() {
    let mut scheduler = LocalScheduler::new();

    let new_actor = simple_actor as fn(_) -> _;
    // Actor 1.
    let actor_entry = scheduler.add_actor();
    let pid1 = actor_entry.pid();
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(Priority::LOW, NoSupervisor, new_actor, actor, inbox, true);
    // Actor 2.
    let actor_entry = scheduler.add_actor();
    let pid2 = actor_entry.pid();
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(Priority::HIGH, NoSupervisor, new_actor, actor, inbox, true);
    // Actor 3.
    let actor_entry = scheduler.add_actor();
    let pid3 = actor_entry.pid();
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        true,
    );

    assert!(scheduler.has_process());
    assert!(scheduler.has_ready_process());

    // Process 2 has a higher priority, should be scheduled first.
    let process2 = scheduler.next_process().unwrap();
    assert_eq!(process2.as_ref().id(), pid2);
    let process3 = scheduler.next_process().unwrap();
    assert_eq!(process3.as_ref().id(), pid3);
    let process1 = scheduler.next_process().unwrap();
    assert_eq!(process1.as_ref().id(), pid1);

    assert!(process1 < process2);
    assert!(process1 < process3);
    assert!(process2 > process1);
    assert!(process2 > process3);
    assert!(process3 > process1);
    assert!(process3 < process2);

    assert_eq!(scheduler.next_process(), None);
}

#[test]
fn add_process() {
    let mut scheduler = LocalScheduler::new();

    let actor_entry = scheduler.add_actor();
    let pid = actor_entry.pid();
    let new_actor = simple_actor as fn(_) -> _;
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        false,
    );

    assert!(scheduler.next_process().is_none());
    assert!(scheduler.has_process());
    assert!(!scheduler.has_ready_process());

    scheduler.mark_ready(pid);
    assert!(scheduler.has_process());
    assert!(scheduler.has_ready_process());
    let process = scheduler.next_process().unwrap();
    assert_eq!(process.as_ref().id(), pid);
}

#[test]
fn add_process_marked_ready() {
    let mut scheduler = LocalScheduler::new();

    let actor_entry = scheduler.add_actor();
    let pid = actor_entry.pid();
    let new_actor = simple_actor as fn(_) -> _;
    let (actor, inbox, _) = init_local_actor_with_inbox(new_actor, ()).unwrap();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        new_actor,
        actor,
        inbox,
        true,
    );

    let process = scheduler.next_process().unwrap();
    scheduler.add_process(process);
    assert!(scheduler.has_process());
    assert!(!scheduler.has_ready_process());

    scheduler.mark_ready(pid);
    assert!(scheduler.has_process());
    assert!(scheduler.has_ready_process());
    let process = scheduler.next_process().unwrap();
    assert_eq!(process.as_ref().id(), pid);
}

#[test]
fn scheduler_run_order() {
    let mut scheduler = LocalScheduler::new();
    let mut runtime_ref = test::runtime();

    // The order in which the processes have been run.
    let run_order = Rc::new(RefCell::new(Vec::new()));

    async fn order_actor(
        _: actor::Context<!>,
        id: usize,
        order: Rc<RefCell<Vec<usize>>>,
    ) -> Result<(), !> {
        order.borrow_mut().push(id);
        Ok(())
    }

    // Add our processes.
    let new_actor = order_actor as fn(_, _, _) -> _;
    let priorities = [Priority::LOW, Priority::NORMAL, Priority::HIGH];
    let mut pids = vec![];
    for (id, priority) in priorities.iter().enumerate() {
        let actor_entry = scheduler.add_actor();
        pids.push(actor_entry.pid());
        let (actor, inbox, _) =
            init_local_actor_with_inbox(new_actor, (id, run_order.clone())).unwrap();
        actor_entry.add(*priority, NoSupervisor, new_actor, actor, inbox, true);
    }

    assert!(scheduler.has_process());
    assert!(scheduler.has_ready_process());

    // Run all processes, should be in order of priority (since there runtimes
    // are equal).
    for _ in 0..3 {
        let mut process = scheduler.next_process().unwrap();
        assert_eq!(
            process.as_mut().run(&mut runtime_ref),
            ProcessResult::Complete
        );
    }
    assert!(!scheduler.has_process());
    assert_eq!(*run_order.borrow(), vec![2usize, 1, 0]);
}

struct TestAssertUnmovedNewActor;

impl NewActor for TestAssertUnmovedNewActor {
    type Message = ();
    type Argument = ();
    type Actor = AssertUnmoved<Pending<Result<(), !>>>;
    type Error = !;
    type Context = context::ThreadLocal;

    fn new(
        &mut self,
        ctx: actor::Context<Self::Message, Self::Context>,
        _: Self::Argument,
    ) -> Result<Self::Actor, Self::Error> {
        // In the test we need the access to the inbox, to achieve that we can't
        // drop the context, so we forget about it here leaking the inbox.
        forget(ctx);
        Ok(AssertUnmoved::new(pending()))
    }
}

#[test]
fn assert_process_unmoved() {
    let mut scheduler = LocalScheduler::new();
    let mut runtime_ref = test::runtime();

    let (actor, inbox, _) = init_local_actor_with_inbox(TestAssertUnmovedNewActor, ()).unwrap();

    let actor_entry = scheduler.add_actor();
    let pid = actor_entry.pid();
    actor_entry.add(
        Priority::NORMAL,
        NoSupervisor,
        TestAssertUnmovedNewActor,
        actor,
        inbox,
        true,
    );

    // Run the process multiple times, ensure it's not moved in the process.
    let mut process = scheduler.next_process().unwrap();
    assert_eq!(
        process.as_mut().run(&mut runtime_ref),
        ProcessResult::Pending
    );
    scheduler.add_process(process);

    scheduler.mark_ready(pid);
    let mut process = scheduler.next_process().unwrap();
    assert_eq!(
        process.as_mut().run(&mut runtime_ref),
        ProcessResult::Pending
    );
    scheduler.add_process(process);

    scheduler.mark_ready(pid);
    let mut process = scheduler.next_process().unwrap();
    assert_eq!(
        process.as_mut().run(&mut runtime_ref),
        ProcessResult::Pending
    );
}
