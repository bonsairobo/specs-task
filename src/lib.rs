//! # Fork-join multitasking for SPECS ECS
//!
//! Instead of hand-rolling state machines to sequence the effects of various ECS systems, spawn
//! tasks as entities and declare explicit temporal dependencies between them.
//!
//! ## Code Examples
//!
//! ### Making task graphs
//!
//! ```compile_fail
//! fn make_static_task_graph(task_maker: &TaskMaker) {
//!     // Any component that implements TaskComponent can be spawned.
//!     let task_graph: TaskGraph = seq!(
//!         @TaskFoo("hello"),
//!         fork!(
//!             @TaskBar { value: 1 },
//!             @TaskBar { value: 2 },
//!             @TaskBar { value: 3 }
//!         ),
//!         @TaskZing("goodbye")
//!     );
//!     task_graph.assemble(task_maker, OnCompletion::Delete);
//! }
//!
//! fn make_dynamic_task_graph(task_maker: &TaskMaker) {
//!     let first = task!(@TaskFoo("hello"));
//!     let mut middle = empty_graph!();
//!     for i in 0..10 {
//!         middle = fork!(middle, @TaskBar { value: i });
//!     }
//!     let last = task!(@TaskZing("goodbye"));
//!     let task_graph: TaskGraph = seq!(first, middle, last);
//!     task_graph.assemble(task_maker, OnCompletion::Delete);
//! }
//! ```
//!
//! ### Building a dispatcher with a `TaskRunnerSystem`
//!
//! ```compile_fail
//! #[derive(Clone, Debug)]
//! struct PushValue {
//!     value: usize,
//! }
//!
//! impl Component for PushValue {
//!     type Storage = VecStorage<Self>;
//! }
//!
//! impl<'a> TaskComponent<'a> for PushValue {
//!     type Data = Write<'a, Vec<usize>>;
//!
//!     fn run(&mut self, data: &mut Self::Data) -> bool {
//!         data.push(self.value);
//!
//!         true
//!     }
//! }
//!
//! fn make_dispatcher() -> Dispatcher {
//!     DispatcherBuilder::new()
//!         .with(
//!             TaskRunnerSystem::<PushValue>::default(),
//!             "push_value",
//!             &[],
//!         )
//!         .with(
//!             TaskManagerSystem,
//!             "task_manager",
//!             &[],
//!         )
//!         .build()
//! }
//! ```
//!
//! ## Data Model
//!
//! Here we expound on the technical details of this module's implementation. For basic usage, see
//! the tests.
//!
//! In this model, every task is some entity. The entity is allowed to have exactly one component
//! that implements `TaskComponent` (it may have other components that don't implement
//! `TaskComponent`). The task will be run to completion by the corresponding `TaskRunnerSystem`.
//!
//! Every task entity is also a node in a (hopefully acyclic) directed graph. An edge `t2 --> t1`
//! means that `t2` cannot start until `t1` has completed.
//!
//! In order for tasks to become unblocked, the `TaskManagerSystem` must run, whence it will
//! traverse the graph, starting at the "final entities", and check for entities that have
//! completed, potentially unblocking their parents. In order for a task to be run, it must be the
//! descendent of a final entity. Entity component tuples become final by calling `finalize` (which
//! adds a `FinalTag` component).
//!
//! Edges can either come from `SingleEdge` or `MultiEdge` components, but you should not use these
//! types directly. You might wonder why we need both types of edges. It's a fair question, because
//! adding the `SingleEdge` concept does not actually make the model capable of representing any
//! semantically new graphs. The reason is efficiency.
//!
//! If you want to implement a fork join like this (note: time is going left to right but the
//! directed edges are going right to left):
//!
//!```
//! r#"       ----- t1.1 <---   ----- t2.1 <---
//!          /               \ /               \
//!      t0 <------ t1.2 <----<------ t2.2 <---- t3
//!          \               / \               /
//!           ----- t1.3 <---   ----- t2.3 <---      "#;
//!```
//!
//! You would actually do this by calling `make_fork` to create two "fork" entities `F1` and `F2`
//! that don't have `TaskComponent`s, but they can have both a `SingleEdge` and a `MultiEdge`. Note
//! that the children on the `MultiEdge` are called "prongs" of the fork.
//!
//!```
//! r#"      single          single          single
//!     t0 <-------- F1 <-------------- F2 <-------- t3
//!                   |                  |
//!          t1.1 <---|          t2.1 <--|
//!          t1.2 <---| multi    t2.2 <--| multi
//!          t1.3 <---|          t2.3 <--|            "#;
//!```
//!
//! The semantics would be such that this graph is equivalent to the one above. Before any of the
//! tasks connected to `F2` by the `MultiEdge` could run, the tasks connected by the `SingleEdge`
//! (`{ t0, t1.1, t1.2, t1.3 }`) would have to be complete. `t3` could only run once all of the
//! descendents of `F2` had completed.
//!
//! The advantages of this scheme are:
//!   - a traversal of the graph starting from `t3` does not visit the same node twice
//!   - it is a bit easier to create fork-join graphs with larger numbers of concurrent tasks
//!   - there are fewer edges for the most common use cases
//!
//! Here's another example with "nested forks" to test your understanding:
//!
//! ```
//! r#"   With fork entities:
//!
//!           t0 <-------------- FA <----- t2
//!                              |
//!                       tx <---|
//!               t1 <--- FB <---|
//!                        |
//!               ty <-----|
//!               tz <-----|
//!
//!       As time orderings:
//!
//!           t0   < { t1, tx, ty, tz } < t2
//!           t1   < { ty, tz }
//!
//!       Induced graph:
//!
//!           t0 <------- tx <------- t2
//!            ^                      |
//!            |      /------ ty <----|
//!            |     v                |
//!            ----- t1 <---- tz <-----          "#;
//! ```
//!
//! ## Macro Usage
//!
//! Every user of this module should create task graphs via the `empty_graph!`, `seq!`, `fork!`, and
//! `task!` macros, which make it easy to construct task graphs correctly. Once a graph is ready,
//! call `assemble` on it to mark the task entities for execution.
//!
//! These systems must be scheduled for tasks to make progress:
//!   - `TaskManagerSystem`
//!   - `TaskRunnerSystem`
//!
//! ## Advanced Usage
//!
//! If you find the `TaskGraph` macros limiting, you can use the `TaskMaker`; these are the building
//! blocks for creating all task graphs, including buggy ones. These functions are totally dynamic
//! in that they deal directly with entities of various archetypes, assuming that the programmer
//! passed in the correct archetypes for the given function.
//!
//! Potential bugs that won't be detected for you:
//!   - leaked orphan entities
//!   - graph cycles
//!   - finalizing an entity that has children
//!   - users manually tampering with the `TaskProgress`, `SingleEdge`, `MultiEdge`, or `FinalTag`
//!     components; these should only be used inside this module
//!

mod components;
mod graph_builder;
mod manager;
mod runner;

pub use components::{
    FinalTag, MultiEdge, OnCompletion, SingleEdge, TaskComponent, TaskMaker, TaskProgress,
};
pub use graph_builder::{Cons, TaskFactory, TaskGraph};
pub use manager::{TaskManager, TaskManagerSystem};
pub use runner::TaskRunnerSystem;

#[cfg(test)]
mod tests {
    use super::*;

    use specs::prelude::*;

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct AlreadyComplete {
        was_run: bool,
    }

    impl Component for AlreadyComplete {
        type Storage = VecStorage<Self>;
    }

    impl<'a> TaskComponent<'a> for AlreadyComplete {
        type Data = ();

        fn run(&mut self, _data: &mut Self::Data) -> bool {
            self.was_run = true;

            true
        }
    }

    #[derive(Clone)]
    struct WriteValue {
        value: usize,
    }

    impl Component for WriteValue {
        type Storage = VecStorage<Self>;
    }

    impl<'a> TaskComponent<'a> for WriteValue {
        type Data = Write<'a, usize>;

        fn run(&mut self, data: &mut Self::Data) -> bool {
            **data = self.value;

            true
        }
    }

    fn set_up<'a, 'b>() -> (World, Dispatcher<'a, 'b>) {
        let mut world = World::new();
        let mut dispatcher = DispatcherBuilder::new()
            .with(
                TaskRunnerSystem::<AlreadyComplete>::default(),
                "already_complete",
                &[],
            )
            .with(
                TaskRunnerSystem::<WriteValue>::default(),
                "write_value",
                &[],
            )
            // For sake of reproducible tests, assume the manager system is the last to run.
            .with(
                TaskManagerSystem,
                "task_manager",
                &["already_complete", "write_value"],
            )
            .build();
        dispatcher.setup(&mut world);

        (world, dispatcher)
    }

    enum MakeSingleTask {
        Finalize(OnCompletion),
        DontFinalize,
    }

    fn make_single_task<'a, T: TaskComponent<'a> + Send + Sync>(
        world: &mut World,
        task: T,
        option: MakeSingleTask,
    ) -> Entity {
        let entity = world.exec(|task_maker: TaskMaker| {
            let task = task_maker.make_task(task);
            if let MakeSingleTask::Finalize(on_completion) = option {
                task_maker.finalize(task, on_completion);
            }

            task
        });
        world.maintain();

        entity
    }

    fn entity_is_complete(world: &mut World, entity: Entity) -> bool {
        world.exec(|task_man: TaskManager| task_man.entity_is_complete(entity))
    }

    #[test]
    fn single_task_not_run_until_finalized() {
        let (mut world, mut dispatcher) = set_up();

        let task = make_single_task(
            &mut world,
            AlreadyComplete::default(),
            MakeSingleTask::DontFinalize,
        );

        // Give the task a chance to get unblocked if there was a bug.
        dispatcher.dispatch(&world);
        dispatcher.dispatch(&world);

        assert_eq!(
            world.read_storage::<AlreadyComplete>().get(task),
            Some(&AlreadyComplete { was_run: false })
        );

        world.exec(|task_maker: TaskMaker| task_maker.finalize(task, OnCompletion::None));
        world.maintain();

        // Unblock the task.
        dispatcher.dispatch(&world);
        // Run the task.
        dispatcher.dispatch(&world);
        // If there was a bug that deleted our entity, this would be necessary to see it.
        world.maintain();

        assert!(entity_is_complete(&mut world, task));
        assert_eq!(
            world.read_storage::<AlreadyComplete>().get(task),
            Some(&AlreadyComplete { was_run: true }),
        );
    }

    #[test]
    fn single_task_deleted_on_completion() {
        let (mut world, mut dispatcher) = set_up();

        let task = make_single_task(
            &mut world,
            AlreadyComplete::default(),
            MakeSingleTask::Finalize(OnCompletion::Delete),
        );

        // Unblock the task.
        dispatcher.dispatch(&world);
        // Run the task, after which it should be deleted.
        dispatcher.dispatch(&world);
        // This needs to be done for the entity deletion to be visible.
        world.maintain();

        assert_eq!(world.entities().is_alive(task), false);
    }

    #[test]
    fn joined_tasks_run_in_order_and_deleted_on_completion() {
        let (mut world, mut dispatcher) = set_up();

        let root = world.exec(|task_maker: TaskMaker| {
            let task_graph: TaskGraph = seq!(
                @WriteValue { value: 1 },
                @WriteValue { value: 2 },
                @WriteValue { value: 3 }
            );

            task_graph.assemble(OnCompletion::Delete, &task_maker)
        });
        world.maintain();

        dispatcher.dispatch(&world);
        dispatcher.dispatch(&world);
        assert_eq!(*world.fetch::<usize>(), 1);
        dispatcher.dispatch(&world);
        assert_eq!(*world.fetch::<usize>(), 2);
        dispatcher.dispatch(&world);
        assert_eq!(*world.fetch::<usize>(), 3);

        world.maintain();
        assert_eq!(world.entities().is_alive(root), false);
    }

    #[test]
    fn all_prongs_of_fork_run_before_join_and_deleted_on_completion() {
        let (mut world, mut dispatcher) = set_up();

        //         ---> t1.1 ---
        //       /               \
        //     t2 ----> t1.2 -----> t0

        let root = world.exec(|task_maker: TaskMaker| {
            let task_graph: TaskGraph = seq!(
                @WriteValue { value: 1 },
                fork!(
                    @WriteValue { value: 2 },
                    @WriteValue { value: 3 }
                ),
                @WriteValue { value: 4 }
            );

            task_graph.assemble(OnCompletion::Delete, &task_maker)
        });
        world.maintain();

        dispatcher.dispatch(&world);
        dispatcher.dispatch(&world);
        assert_eq!(*world.fetch::<usize>(), 1);
        dispatcher.dispatch(&world);
        let cur_value = *world.fetch::<usize>();
        assert!(cur_value == 2 || cur_value == 3);
        dispatcher.dispatch(&world);
        assert_eq!(*world.fetch::<usize>(), 4);

        world.maintain();
        assert_eq!(world.entities().is_alive(root), false);
    }
}
