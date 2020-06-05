use crate::components::*;

use specs::{prelude::*, storage::GenericReadStorage, world::LazyBuilder};

/// `SystemData` for all task-related operations. Generic over the type of `TaskProgress` storage
/// so that it can be used in multiple contexts.
#[derive(SystemData)]
pub struct TaskData<'a, P> {
    lazy: Read<'a, LazyUpdate>,
    pub(crate) entities: Entities<'a>,
    pub(crate) progress: P,
    pub(crate) single_edges: ReadStorage<'a, SingleEdge>,
    pub(crate) multi_edges: ReadStorage<'a, MultiEdge>,
}

/// `SystemData` for creating, polling, and deleting graphs of task entities. Only has read access
/// to task components and does all creation/deletion lazily.
pub type TaskUser<'a> = TaskData<'a, ReadStorage<'a, TaskProgress>>;

impl<'a, P> TaskData<'a, P>
where
    P: 'a + GenericReadStorage<Component=TaskProgress>,
    &'a P: Join,
{
    /// Like `make_task`, but use `entity` for tracking the task components. This can make it easier
    /// to manage tasks coupled with a specific entity (rather than storing a separate task entity
    /// in a component).
    pub fn make_task_with_entity<'b, T: TaskComponent<'b> + Send + Sync>(
        &self, entity: Entity, task: T
    ) {
        LazyBuilder {
            entity,
            lazy: &self.lazy,
        }
        .with(task)
        .with(TaskProgress::default())
        .build();
        log::debug!("Created task {:?}", entity);
    }

    /// Create a new task entity with the given `TaskComponent`. The task will not make progress
    /// until it is either finalized or the descendent of a finalized entity.
    pub fn make_task<'b, T: TaskComponent<'b> + Send + Sync>(&self, task: T) -> Entity {
        let entity = self
            .lazy
            .create_entity(&self.entities)
            .with(task)
            .with(TaskProgress::default())
            .build();
        log::debug!("Created task {:?}", entity);

        entity
    }

    /// Same as `make_task_with_entity`, but also finalizes the task.
    pub fn make_final_task_with_entity<'b, T: TaskComponent<'b> + Send + Sync>(
        &self,
        entity: Entity,
        task: T,
        on_completion: OnCompletion,
    ) -> Entity {
        self.make_task_with_entity(entity, task);
        self.finalize(entity, on_completion);

        entity
    }

    /// Same as `make_task`, but also finalizes the task.
    pub fn make_final_task<'b, T: TaskComponent<'b> + Send + Sync>(
        &self,
        task: T,
        on_completion: OnCompletion,
    ) -> Entity {
        let task_entity = self.make_task(task);
        self.finalize(task_entity, on_completion);

        task_entity
    }

    /// Create a new fork entity with no children.
    pub fn make_fork(&self) -> Entity {
        let entity = self
            .lazy
            .create_entity(&self.entities)
            .with(MultiEdge::default())
            .build();
        log::debug!("Created fork {:?}", entity);

        entity
    }

    /// Add `prong` as a child on the `MultiEdge` of `fork_entity`.
    pub fn add_prong(&self, fork_entity: Entity, prong: Entity) {
        self.lazy.exec_mut(move |world| {
            let mut multi_edges = world.write_component::<MultiEdge>();
            let multi_edge = multi_edges.get_mut(fork_entity).unwrap_or_else(|| {
                panic!(
                    "Tried to add prong {:?} to non-fork entity {:?}",
                    prong, fork_entity
                )
            });
            multi_edge.add_child(prong);
        });
    }

    /// Creates a `SingleEdge` from `parent` to `child`. Creates a fork-join if `parent` is a fork.
    pub fn join(&self, parent: Entity, child: Entity) {
        self.lazy.exec_mut(move |world| {
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
    pub fn finalize(&self, entity: Entity, on_completion: OnCompletion) {
        self.lazy.exec_mut(move |world| {
            let mut finalized = world.write_component::<FinalTag>();
            finalized
                .insert(entity, FinalTag { on_completion })
                .unwrap();
        });
    }

    /// Returns true iff the task was seen as complete on the last run of the `TaskManagerSystem`.
    pub fn task_is_complete(&self, task: Entity) -> bool {
        if let Some(progress) = self.progress.get(task) {
            progress.is_complete()
        } else {
            // Task entity may not have a TaskProgress component yet if it's constructed lazily.
            false
        }
    }

    /// Returns true iff all of `entity`'s children are complete.
    fn fork_is_complete(&self, multi_children: &[Entity]) -> bool {
        // We know that a fork's SingleEdge child is complete if any of the MultiEdge children are
        // complete.
        for child in multi_children.iter() {
            if !self.entity_is_complete(*child) {
                return false;
            }
        }

        true
    }

    /// Tells you whether a fork or a task entity is complete.
    pub fn entity_is_complete(&self, entity: Entity) -> bool {
        // Only fork entities can have `MultiEdge`s. If the entity is being created lazily, we won't
        // know if it's a fork or task, but we won't consider it complete regardless.
        if let Some(MultiEdge { children }) = self.multi_edges.get(entity) {
            self.fork_is_complete(&children)
        } else {
            self.task_is_complete(entity)
        }
    }

    /// Returns the number of tasks that haven't yet completed.
    pub fn count_tasks_in_progress(&'a self) -> usize {
        (&self.progress).join().count()
    }

    /// Deletes only the descendent entities of `entity`, but leaves `entity` alive.
    pub fn delete_descendents(&self, entity: Entity) {
        if let Some(MultiEdge { children }) = self.multi_edges.get(entity) {
            for child in children.iter() {
                self.delete_entity_and_descendents(*child);
            }
        }
        if let Some(SingleEdge { child }) = self.single_edges.get(entity) {
            self.delete_entity_and_descendents(*child);
        }
    }

    /// Deletes `entity` and all of its descendents.
    pub fn delete_entity_and_descendents(&self, entity: Entity) {
        // Support async deletion. If a child is deleted, we assume all of its descendants were also
        // deleted.
        if !self.entities.is_alive(entity) {
            return;
        }

        self.delete_descendents(entity);
        log::debug!("Deleting {:?}", entity);
        self.entities.delete(entity).unwrap();
    }

    /// Deletes the entity and descendents if they are all complete. Returns true iff the entity and
    /// all descendents are complete.
    pub fn delete_if_complete(&self, entity: Entity) -> bool {
        if self.entity_is_complete(entity) {
            self.delete_entity_and_descendents(entity);

            true
        } else {
            false
        }
    }
}
