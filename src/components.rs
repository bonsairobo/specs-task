use specs::{prelude::*, world::LazyBuilder};
use std::sync::atomic::{AtomicBool, Ordering};

/// An ephemeral component that needs access to `SystemData` to run some task. Will be run by the
/// `TaskRunnerSystem<T>` until `run` returns `true`.
///
/// Note: `TaskComponent::Data` isn't allowed to contain `Storage<TaskComponent>`, since the
/// `TaskRunnerSystem` already uses that resource and borrows it mutably while calling
/// `TaskComponent::run`. If you really need access to `Storage<TaskComponent>`, you can
/// safely use the `LazyUpdate` resource for that.
pub trait TaskComponent<'a>: Component {
    type Data: SystemData<'a>;

    /// Returns `true` iff the task is complete.
    fn run(&mut self, data: &mut Self::Data) -> bool;
}

// As long as an entity has this component, it will be considered by the `TaskRunnerSystem`.
#[doc(hidden)]
#[derive(Default)]
pub struct TaskProgress {
    pub(crate) is_complete: AtomicBool,
    pub(crate) is_unblocked: bool,
}

impl Component for TaskProgress {
    type Storage = VecStorage<Self>;
}

impl TaskProgress {
    pub(crate) fn is_complete(&self) -> bool {
        self.is_complete.load(Ordering::Relaxed)
    }

    pub(crate) fn complete(&self) {
        self.is_complete.store(true, Ordering::Relaxed);
    }

    pub(crate) fn unblock(&mut self) {
        self.is_unblocked = true;
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct SingleEdge {
    pub(crate) child: Entity,
}

impl Component for SingleEdge {
    type Storage = VecStorage<Self>;
}

#[doc(hidden)]
#[derive(Clone, Default)]
pub struct MultiEdge {
    pub(crate) children: Vec<Entity>,
}

impl MultiEdge {
    fn add_child(&mut self, entity: Entity) {
        self.children.push(entity);
    }
}

impl Component for MultiEdge {
    type Storage = VecStorage<Self>;
}

/// What to do to a final task and its descendents when it they complete.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum OnCompletion {
    None,
    Delete,
    DeleteDescendents,
}

impl Default for OnCompletion {
    fn default() -> Self {
        OnCompletion::None
    }
}

#[doc(hidden)]
#[derive(Clone, Copy, Default)]
pub struct FinalTag {
    pub(crate) on_completion: OnCompletion,
}

impl Component for FinalTag {
    type Storage = VecStorage<Self>;
}

/// Like `make_task`, but use `entity` for tracking the task components. This can make it easier
/// to manage tasks coupled with a specific entity (rather than storing a separate task entity
/// in a component).
pub fn make_task_with_entity<'a, T: TaskComponent<'a>>(lazy: &LazyUpdate, entity: Entity, task: T) {
    LazyBuilder { entity, lazy }
        .with(task)
        .with(TaskProgress::default())
        .build();
    log::debug!("Created task {:?}", entity);
}

/// Create a new task entity with the given `TaskComponent`. The task will not make progress
/// until it is either finalized or the descendent of a finalized entity.
pub fn make_task<'a, T: TaskComponent<'a>>(
    lazy: &LazyUpdate,
    entities: &Entities,
    task: T,
) -> Entity {
    let entity = lazy
        .create_entity(entities)
        .with(task)
        .with(TaskProgress::default())
        .build();
    log::debug!("Created task {:?}", entity);

    entity
}

/// Same as `make_task_with_entity`, but also finalizes the task.
pub fn make_final_task_with_entity<'a, T: TaskComponent<'a>>(
    lazy: &LazyUpdate,
    entity: Entity,
    task: T,
    on_completion: OnCompletion,
) -> Entity {
    make_task_with_entity(lazy, entity, task);
    finalize(lazy, entity, on_completion);

    entity
}

/// Same as `make_task`, but also finalizes the task.
pub fn make_final_task<'a, T: TaskComponent<'a>>(
    lazy: &LazyUpdate,
    entities: &Entities,
    task: T,
    on_completion: OnCompletion,
) -> Entity {
    let task_entity = make_task(lazy, entities, task);
    finalize(lazy, task_entity, on_completion);

    task_entity
}

/// Create a new fork entity with no children.
pub fn make_fork(lazy: &LazyUpdate, entities: &Entities) -> Entity {
    let entity = lazy
        .create_entity(entities)
        .with(MultiEdge::default())
        .build();
    log::debug!("Created fork {:?}", entity);

    entity
}

/// Add `prong` as a child on the `MultiEdge` of `fork_entity`.
pub fn add_prong(lazy: &LazyUpdate, fork_entity: Entity, prong: Entity) {
    lazy.exec_mut(move |world| {
        let mut multi_edges = world.write_component::<MultiEdge>();
        let multi_edge = multi_edges
            .get_mut(fork_entity)
            .unwrap_or_else(|| {
                panic!(
                    "Tried to add prong {:?} to non-fork entity {:?}",
                    prong, fork_entity
                )
            });
        multi_edge.add_child(prong);
    });
}

/// Creates a `SingleEdge` from `parent` to `child`. Creates a fork-join if `parent` is a fork.
pub fn join(lazy: &LazyUpdate, parent: Entity, child: Entity) {
    lazy.exec_mut(move |world| {
        let mut single_edges = world.write_component::<SingleEdge>();
        if let Some(edge) = single_edges.get_mut(parent) {
            panic!(
                "Attempted to make task {:?} child of {:?}, but task {:?} already has child {:?}",
                child, parent, parent, edge.child
            );
        } else {
            single_edges.insert(parent, SingleEdge { child }).unwrap();
        }
    });
}

/// Mark `entity` as final. This will make all of `entity`'s descendents visible to the
/// `TaskManagerSystem`, allowing them to make progress. If `OnCompletion::Delete`, then
/// `entity` and all of its descendents will be deleted when `entity` is complete (and hence the
/// entire graph is complete). Otherwise, you need to clean up the entities your self by calling
/// `delete_entity_and_descendents`. God help you if you leak an orphaned entity.
pub fn finalize(lazy: &LazyUpdate, entity: Entity, on_completion: OnCompletion) {
    lazy.exec_mut(move |world| {
        let mut finalized = world.write_component::<FinalTag>();
        finalized.insert(entity, FinalTag { on_completion }).unwrap();
    });
}
