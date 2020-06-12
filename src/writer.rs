use crate::{components::*, user::TaskUser, TaskData};

use specs::prelude::*;

/// Creates and modifies task graph entities. Effects are immediate, not lazy like `TaskUser`, so
/// this requires using `WriteStorage` for task graph components.
pub type TaskWriter<'a> = TaskData<'a,
    WriteStorage<'a, TaskProgress>,
    WriteStorage<'a, SingleEdge>,
    WriteStorage<'a, MultiEdge>,
    WriteStorage<'a, FinalTag>,
>;

impl<'a> TaskWriter<'a> {
    /// Like `make_task`, but use `entity` for tracking the task components.
    pub fn make_task_with_entity<'b, T: TaskComponent<'b> + Send + Sync>(
        &mut self, entity: Entity, task: T, task_storage: &mut WriteStorage<T>,
    ) {
        task_storage.insert(entity, task).unwrap();
        self.progress.insert(entity, TaskProgress::default()).unwrap();
        log::debug!("Created task {:?}", entity);
    }

    /// Create a new task entity with the given `TaskComponent`. The task will not make progress
    /// until it is either finalized or the descendent of a finalized entity.
    pub fn make_task<'b, T: TaskComponent<'b> + Send + Sync>(
        &mut self, task: T, task_storage: &mut WriteStorage<T>
    ) -> Entity {
        let entity = self.entities.create();
        self.make_task_with_entity(entity, task, task_storage);
        log::debug!("Created task {:?}", entity);

        entity
    }

    /// Same as `make_task_with_entity`, but also finalizes the task.
    pub fn make_final_task_with_entity<'b, T: TaskComponent<'b> + Send + Sync>(
        &mut self,
        entity: Entity,
        task: T,
        on_completion: OnCompletion,
        task_storage: &mut WriteStorage<T>,
    ) -> Entity {
        self.make_task_with_entity(entity, task, task_storage);
        self.finalize(entity, on_completion);

        entity
    }

    /// Same as `make_task`, but also finalizes the task.
    pub fn make_final_task<'b, T: TaskComponent<'b> + Send + Sync>(
        &mut self,
        task: T,
        on_completion: OnCompletion,
        task_storage: &mut WriteStorage<T>,
    ) -> Entity {
        let task_entity = self.make_task(task, task_storage);
        self.finalize(task_entity, on_completion);

        task_entity
    }

    /// Create a new fork entity with no children.
    pub fn make_fork(&mut self) -> Entity {
        let entity = self.entities.create();
        self.multi_edges.insert(entity, MultiEdge::default()).unwrap();
        log::debug!("Created fork {:?}", entity);

        entity
    }

    /// Add `prong` as a child on the `MultiEdge` of `fork_entity`.
    pub fn add_prong(&mut self, fork_entity: Entity, prong: Entity) {
        let multi_edge = self.multi_edges.get_mut(fork_entity).unwrap_or_else(|| {
            panic!(
                "Tried to add prong {:?} to non-fork entity {:?}",
                prong, fork_entity
            )
        });
        multi_edge.add_child(prong);
    }

    /// Creates a `SingleEdge` from `parent` to `child`. Creates a fork-join if `parent` is a fork.
    pub fn join(&mut self, parent: Entity, child: Entity) {
        if let Some(edge) = self.single_edges.get_mut(parent) {
            panic!(
                "Attempted to make task {:?} child of {:?}, but task {:?} already has child {:?}",
                child, parent, parent, edge.child
            );
        } else {
            self.single_edges.insert(parent, SingleEdge { child }).unwrap();
        }
    }

    /// Mark `entity` as final. This will make all of `entity`'s descendents visible to the
    /// `TaskManagerSystem`, allowing them to make progress. If `OnCompletion::Delete`, then
    /// `entity` and all of its descendents will be deleted when `entity` is complete (and hence the
    /// entire graph is complete). Otherwise, you need to clean up the entities your self by calling
    /// `delete_entity_and_descendents`. God help you if you leak an orphaned entity.
    pub fn finalize(&mut self, entity: Entity, on_completion: OnCompletion) {
        self.final_tags.insert(entity, FinalTag { on_completion }).unwrap();
    }
}

/// Implemented by all nodes of a `TaskGraph`. Has a blanket impl that should work for most
/// `TaskComponent`s.
pub trait TaskFactory {
    fn create_task(&self, user: &TaskUser) -> Entity;
}

impl<'a, T: 'static + Clone + TaskComponent<'a> + Send + Sync> TaskFactory for T {
    fn create_task(&self, user: &TaskUser) -> Entity {
        user.make_task_lazy(self.clone())
    }
}

// PERF: Cons requires a lot of heap allocations, but this choice was made to avoid using recursive
// types which prevent assigning different graphs to a single variable (e.g. accumulating a graph in
// a loop).

/// Implementation detail of `TaskGraph`. Note that two trees may be unequal yet equivalent in how
/// they `assemble`, for example, `Cons::Seq(x, Cons::Seq(y, z)) != Cons::Seq(Cons::Seq(x, y), z)`,
/// but they both assemble into a sequence `x -> y -> z`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cons<T> {
    Fork(Box<Cons<T>>, Box<Cons<T>>),
    Seq(Box<Cons<T>>, Box<Cons<T>>),
    Task(T),
    Nil, // currently required to support graph accumulation
}

impl<T> Cons<T> {
    fn remove_nil(self) -> Self {
        match self {
            Cons::Seq(head, tail) => match (*head, *tail) {
                (Cons::Nil, Cons::Nil) => Cons::Nil,
                (Cons::Nil, t) => t.remove_nil(),
                (h, Cons::Nil) => h.remove_nil(),
                (h, t) => Cons::Seq(Box::new(h.remove_nil()), Box::new(t.remove_nil())),
            },
            Cons::Fork(head, tail) => match (*head, *tail) {
                (Cons::Nil, Cons::Nil) => Cons::Nil,
                (Cons::Nil, t) => t.remove_nil(),
                (h, Cons::Nil) => h.remove_nil(),
                (h, t) => Cons::Fork(Box::new(h.remove_nil()), Box::new(t.remove_nil())),
            },
            Cons::Task(t) => Cons::Task(t),
            Cons::Nil => Cons::Nil,
        }
    }
}

/// A node of the binary tree grammar that describes a task graph. `Cons::Seq` lists represent
/// sequential execution of tasks. `Cons::Fork` lists represent concurrent execution of tasks. The
/// leaves of the tree are `Cons::Task`s.
pub type TaskGraph = Cons<Box<dyn TaskFactory + Send + Sync>>;

impl Cons<Box<dyn TaskFactory + Send + Sync>> {
    fn _assemble(self, fork: Option<Entity>, user: &TaskUser) -> (Entity, Entity) {
        match self {
            Cons::Seq(head, tail) => {
                let (head_first_entity, head_last_entity) = head._assemble(None, user);
                let (tail_first_entity, tail_last_entity) = tail._assemble(None, user);
                user.join_lazy(tail_first_entity, head_last_entity);

                (head_first_entity, tail_last_entity)
            }
            Cons::Fork(head, tail) => {
                let fork_entity = if let Some(e) = fork {
                    e
                } else {
                    user.make_fork_lazy()
                };

                let (_, head_last_entity) = head._assemble(Some(fork_entity), user);
                let (_, tail_last_entity) = tail._assemble(Some(fork_entity), user);

                // Any decendents reachable only via Cons::Fork are considered prongs. If a
                // descendent is a Cons::Seq, then the prong only connects at the "last" entity of
                // the sequence.
                if head_last_entity != fork_entity {
                    user.add_prong_lazy(fork_entity, head_last_entity);
                }
                if tail_last_entity != fork_entity {
                    user.add_prong_lazy(fork_entity, tail_last_entity);
                }

                (fork_entity, fork_entity)
            }
            Cons::Task(task) => {
                let task_entity = task.create_task(user);

                (task_entity, task_entity)
            }
            Cons::Nil => panic!("Tried to assemble Cons::Nil, which should always be removed."),
        }
    }

    /// Lazily create all of the task graph entities and connect them with edges. Tasks won't exist
    /// until calling `World::maintain`.
    pub fn assemble(self, on_completion: OnCompletion, user: &TaskUser) -> Option<Entity> {
        let s = self.remove_nil();
        if let Cons::Nil = s {
            return None;
        }

        let (_first_entity, last_entity) = s._assemble(None, user);
        user.finalize_lazy(last_entity, on_completion);

        Some(last_entity)
    }
}

// TODO: Get rid of the "@" that precedes every task expression. I am bad at macros, please help!

/// Make a task graph without any tasks. This is used as the initial value for accumulating graphs
/// dynamically.
#[macro_export]
macro_rules! empty_graph {
    () => {
        Cons::Nil
    };
}

/// Make a single-node `TaskGraph`.
#[macro_export]
macro_rules! task {
    (@$task:expr) => {
        Cons::Task(Box::new($task))
    };
}

// TODO: deduplicate these definitions that are mostly the same

/// Returns a `TaskGraph` that executes the argument list of `TaskGraphs` concurrently.
#[macro_export]
macro_rules! fork {
    (@$head:expr, $($tail:tt)*) => (
        Cons::Fork(Box::new(fork!(@$head)), Box::new(fork!($($tail)*)))
    );
    ($head:expr, $($tail:tt)*) => (
        Cons::Fork(Box::new(fork!($head)), Box::new(fork!($($tail)*)))
    );
    (@$task:expr) => (
        Cons::Task(Box::new($task))
    );
    ($head:expr) => ( $head );
}

/// Returns a `TaskGraph` that executes the argument list of `TaskGraphs` sequentially.
#[macro_export]
macro_rules! seq {
    (@$head:expr, $($tail:tt)*) => (
        Cons::Seq(Box::new(seq!(@$head)), Box::new(seq!($($tail)*)))
    );
    ($head:expr, $($tail:tt)*) => (
        Cons::Seq(Box::new(seq!($head)), Box::new(seq!($($tail)*)))
    );
    (@$task:expr) => (
        Cons::Task(Box::new($task))
    );
    ($head:expr) => ( $head );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Foo(u32);

    #[test]
    fn task_graph_macro_fork_two() {
        let x = fork!(@Foo(1u32), @Foo(2u32));
        assert_eq!(
            x,
            Cons::Fork(
                Box::new(Cons::Task(Box::new(Foo(1u32)))),
                Box::new(Cons::Task(Box::new(Foo(2u32)))),
            )
        );
    }

    #[test]
    fn task_graph_macro_fork_three() {
        let x = fork!(@Foo(1u32), @Foo(2u32), @Foo(3u32));
        assert_eq!(
            x,
            Cons::Fork(
                Box::new(Cons::Task(Box::new(Foo(1u32)))),
                Box::new(Cons::Fork(
                    Box::new(Cons::Task(Box::new(Foo(2u32)))),
                    Box::new(Cons::Task(Box::new(Foo(3u32)))),
                )),
            )
        );
    }

    #[test]
    fn task_graph_macro_nested_fork() {
        let x: Cons<Box<Foo>> = fork!(@Foo(1u32), fork!(@Foo(2u32), @Foo(3u32)));
        assert_eq!(
            x,
            Cons::Fork(
                Box::new(Cons::Task(Box::new(Foo(1u32)))),
                Box::new(Cons::Fork(
                    Box::new(Cons::Task(Box::new(Foo(2u32)))),
                    Box::new(Cons::Task(Box::new(Foo(3u32)))),
                )),
            )
        );
    }

    #[test]
    fn task_graph_macro_many_nested() {
        let x = seq!(
            @Foo(1u32),
            fork!(
                seq!(@Foo(2u32), @Foo(3u32), @Foo(4u32)),
                @Foo(5u32),
                @Foo(6u32)
            ),
            @Foo(7u32)
        );
        let y: Cons<Box<Foo>> = Cons::Seq(
            Box::new(Cons::Task(Box::new(Foo(1u32)))),
            Box::new(Cons::Seq(
                Box::new(Cons::Fork(
                    Box::new(Cons::Seq(
                        Box::new(Cons::Task(Box::new(Foo(2u32)))),
                        Box::new(Cons::Seq(
                            Box::new(Cons::Task(Box::new(Foo(3u32)))),
                            Box::new(Cons::Task(Box::new(Foo(4u32)))),
                        )),
                    )),
                    Box::new(Cons::Fork(
                        Box::new(Cons::Task(Box::new(Foo(5u32)))),
                        Box::new(Cons::Task(Box::new(Foo(6u32)))),
                    )),
                )),
                Box::new(Cons::Task(Box::new(Foo(7u32)))),
            )),
        );
        assert_eq!(x, y);
    }

    #[test]
    fn remove_nil_from_left_fork() {
        let x = fork!(Cons::Nil, @Foo(1));
        assert_eq!(x.remove_nil(), task!(@Foo(1)));
    }

    #[test]
    fn remove_nil_from_right_fork() {
        let x = fork!(@Foo(1), Cons::Nil);
        assert_eq!(x.remove_nil(), task!(@Foo(1)));
    }

    #[test]
    fn remove_all_nils_nested_fork() {
        let x = fork!(Cons::Nil, fork!(Cons::Nil, @Foo(1)));
        assert_eq!(x.remove_nil(), task!(@Foo(1)));
    }

    #[test]
    fn accumulate_sequence_in_loop() {
        let mut s = empty_graph!();
        for i in 0..4 {
            s = seq!(s, @Foo(i));
        }
        // Unfortunately removing nils puts the tree in an equivalent but not equal shape.
        assert_eq!(
            s.remove_nil(),
            seq!(seq!(seq!(@Foo(0), @Foo(1)), @Foo(2)), @Foo(3))
        );
    }
}
