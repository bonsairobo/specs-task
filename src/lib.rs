//! A multitasking module that supports the fork-join model. Implemented on top of SPECS ECS.
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
//! descendent of a final entity. Entities become final by calling `TaskManager::finalize`.
//!
//! Edges can either come from `SingleEdge` or `MultiEdge` components, but you should not use these
//! types directly. You might wonder why we need both. It's a fair question, because adding the
//! `SingleEdge` concept does not actually make the model capable of representing any semantically
//! new graphs. The reason is efficiency.
//!
//! If you want to implement a fork join like this:
//!
//!```
//! r#"     ---> t1.1 ---
//!       /               \
//!     t2 ----> t1.2 -----> t0
//!       \               /
//!         ---> t1.3 ---       "#;
//!```
//!
//! You would actually do this by calling `TaskManager::make_fork` to create a "fork" entity called
//! `F` that doesn't have a `TaskComponent`, but it has a `SingleEdge` from `t2` to `t0`, and a
//! `MultiEdge` from `t2` to `{ t1.1, t1.2, t1.3 }`. Note that the children on the `MultiEdge` are
//! called "prongs" of the fork.
//!
//!```
//! r#"  t2 --> F --> t0
//!             |
//!             | --> t1.1
//!             | --> t1.2
//!             | --> t1.3  "#;
//!```
//!
//! The semantics would be such that this graph is equivalent to the one above. Before any of the
//! tasks connected to `F` by the `MultiEdge` could run, the task connected by the `SingleEdge`
//! (`t0`) would have to be complete. `t2` could only run once all of the children of `F` had
//! completed.
//!
//! The advantages of this scheme are:
//!   - a traversal of the graph starting from `t2` does not visit the same node twice
//!   - it is a bit easier to create fork-join graphs with larger numbers of concurrent tasks
//!   - there are fewer edges for the most common use cases
//!
//! Every user of this module should use it via the `TaskManager`. It will enforce certain
//! invariants about the kinds of entities that can be constructed. For example, any entity with a
//! `MultiEdge` component is considered a "fork entity", and it is not allowed to have a
//! `TaskComponent` or a `TaskProgress`. Therefore, if you want a task to have multiple children, it
//! must do so via a fork entity.
//!
//! These systems must be dispatched for tasks to make progress:
//!   - `TaskManagerSystem`
//!   - `TaskRunnerSystem` for every `T: TaskRunner` used
//!
//! This module can be dangerous when used improperly due to the dynamic nature of SPECS. Potential
//! bugs not handled by this module:
//!   - leaked orphan entities
//!   - graph cycles
//!   - DO NOT manually touch the storages for task module components! Always go through the
//!     `TaskManager`.

mod task_manager;
mod task_runner;

pub use task_manager::{
    AlreadyJoined, FinalTag, MultiEdge, SingleEdge, TaskManager, TaskManagerSystem,
    UnexpectedEntity,
};
pub use task_runner::TaskRunnerSystem;

use specs::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// An ephemeral component that needs access to `SystemData` to run some task. Will be run until
/// `is_complete` by the `TaskRunnerSystem<T>`.
///
/// Note: `TaskComponent::Data` isn't allowed to contain `Storage<TaskComponent>`, since the
/// `TaskRunnerSystem` already uses that resource and borrows it mutably while calling
/// `TaskComponent::run`.
pub trait TaskComponent<'a>: Component {
    type Data: SystemData<'a>;

    /// Returns `true` iff the task is complete.
    fn run(&mut self, data: &mut Self::Data) -> bool;
}

// As long as an entity has this component, it will be considered by the `TaskRunnerSystem`.
/// WARNING: only public because `TaskManager` is public. DO NOT USE.
#[derive(Default)]
pub struct TaskProgress {
    is_complete: AtomicBool,
    is_unblocked: bool,
}

impl Component for TaskProgress {
    type Storage = VecStorage<Self>;
}

impl TaskProgress {
    fn is_complete(&self) -> bool {
        self.is_complete.load(Ordering::Relaxed)
    }

    fn complete(&self) {
        self.is_complete.store(true, Ordering::Relaxed);
    }

    fn unblock(&mut self) {
        self.is_unblocked = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Eq, PartialEq)]
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
        Finalize(bool),
        DontFinalize,
    }

    fn make_single_task<'a, T: TaskComponent<'a>>(
        world: &mut World,
        task: T,
        option: MakeSingleTask,
    ) -> Entity {
        world.exec(
            |(entities, mut task_man, mut tasks): (Entities, TaskManager, WriteStorage<T>)| {
                let task = task_man.make_task(task, &entities, &mut tasks);
                if let MakeSingleTask::Finalize(delete_on_completion) = option {
                    task_man.finalize(task, delete_on_completion);
                }

                task
            },
        )
    }

    fn make_fork(world: &mut World) -> Entity {
        world
            .exec(|(entities, mut task_man): (Entities, TaskManager)| task_man.make_fork(&entities))
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

        world.exec(|mut task_man: TaskManager| task_man.finalize(task, false));

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
            MakeSingleTask::Finalize(true),
        );

        // Unblock the task.
        dispatcher.dispatch(&world);
        // Run the task, after which it should be deleted.
        dispatcher.dispatch(&world);
        // This needs to be done for the entity deletion to be visible.
        world.maintain();

        assert!(entity_is_complete(&mut world, task));
        assert_eq!(world.entities().is_alive(task), false);
    }

    #[test]
    fn joined_tasks_run_in_order_and_deleted_on_completion() {
        let (mut world, mut dispatcher) = set_up();

        let task1 = make_single_task(
            &mut world,
            WriteValue { value: 1 },
            MakeSingleTask::DontFinalize,
        );
        let task2 = make_single_task(
            &mut world,
            WriteValue { value: 2 },
            MakeSingleTask::DontFinalize,
        );
        let task3 = make_single_task(
            &mut world,
            WriteValue { value: 3 },
            MakeSingleTask::DontFinalize,
        );

        world.exec(|mut task_man: TaskManager| {
            task_man.join(task3, task2).unwrap();
            task_man.join(task2, task1).unwrap();
            task_man.finalize(task3, true);
        });

        dispatcher.dispatch(&world);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, task1));
        assert_eq!(*world.fetch::<usize>(), 1);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, task2));
        assert_eq!(*world.fetch::<usize>(), 2);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, task3));
        assert_eq!(*world.fetch::<usize>(), 3);

        world.maintain();
        assert_eq!(world.entities().is_alive(task1), false);
        assert_eq!(world.entities().is_alive(task2), false);
        assert_eq!(world.entities().is_alive(task3), false);
    }

    #[test]
    fn all_prongs_of_fork_run_before_join_and_deleted_on_completion() {
        let (mut world, mut dispatcher) = set_up();

        //         ---> t1.1 ---
        //       /               \
        //     t2 ----> t1.2 -----> t0

        let fork = make_fork(&mut world);
        let initial_task = make_single_task(
            &mut world,
            WriteValue { value: 1 },
            MakeSingleTask::DontFinalize,
        );
        let prong1_task = make_single_task(
            &mut world,
            WriteValue { value: 2 },
            MakeSingleTask::DontFinalize,
        );
        let prong2_task = make_single_task(
            &mut world,
            WriteValue { value: 3 },
            MakeSingleTask::DontFinalize,
        );
        let join_task = make_single_task(
            &mut world,
            WriteValue { value: 4 },
            MakeSingleTask::DontFinalize,
        );

        world.exec(|mut task_man: TaskManager| {
            task_man.join(fork, initial_task).unwrap();
            task_man.add_prong(fork, prong1_task).unwrap();
            task_man.add_prong(fork, prong2_task).unwrap();
            task_man.join(join_task, fork).unwrap();
            task_man.finalize(join_task, true);
        });

        dispatcher.dispatch(&world);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, initial_task));
        assert_eq!(*world.fetch::<usize>(), 1);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, prong1_task));
        assert!(entity_is_complete(&mut world, prong2_task));
        let cur_value = *world.fetch::<usize>();
        assert!(cur_value == 2 || cur_value == 3);
        dispatcher.dispatch(&world);
        assert!(entity_is_complete(&mut world, join_task));
        assert_eq!(*world.fetch::<usize>(), 4);

        world.maintain();
        assert_eq!(world.entities().is_alive(initial_task), false);
        assert_eq!(world.entities().is_alive(prong1_task), false);
        assert_eq!(world.entities().is_alive(prong2_task), false);
        assert_eq!(world.entities().is_alive(join_task), false);
    }

    #[test]
    fn test_cant_add_prong_to_task() {
        let (mut world, _) = set_up();

        let task = make_single_task(
            &mut world,
            AlreadyComplete::default(),
            MakeSingleTask::Finalize(true),
        );
        let fork = make_fork(&mut world);

        world.exec(|mut task_man: TaskManager| {
            assert_eq!(
                task_man.add_prong(task, fork),
                Err(UnexpectedEntity::ExpectedForkEntity(task)),
            );
        });
    }

    #[test]
    fn test_already_joined_error() {
        let (mut world, _) = set_up();

        let task1 = make_single_task(
            &mut world,
            AlreadyComplete::default(),
            MakeSingleTask::Finalize(true),
        );
        let task2 = make_single_task(
            &mut world,
            AlreadyComplete::default(),
            MakeSingleTask::Finalize(true),
        );

        world.exec(|mut task_man: TaskManager| {
            assert!(task_man.join(task1, task2).is_ok());
            assert_eq!(
                task_man.join(task1, task2),
                Err(AlreadyJoined {
                    parent: task1,
                    already_child: task2,
                    new_child: task2,
                }),
            );
        });
    }
}
