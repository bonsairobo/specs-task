use crate::{components::*, graph_builder::TaskBuilder};

use specs::{prelude::*, world::LazyBuilder};

/// `SystemData` for all read-only task-related operations. Can create and modify tasks lazily.
#[derive(SystemData)]
pub struct TaskUser<'a> {
    pub(crate) entities: Entities<'a>,
    lazy: Read<'a, LazyUpdate>,
    progress: ReadStorage<'a, TaskProgress>,
    single_edges: ReadStorage<'a, SingleEdge>,
    multi_edges: ReadStorage<'a, MultiEdge>,
}

impl<'a> TaskUser<'a> {
    /// Like `make_task`, but use `entity` for tracking the task components. This can make it easier
    /// to manage tasks coupled with a specific entity (rather than storing a separate task entity
    /// in a component).
    pub fn make_task_with_entity_lazy<'b, T: TaskComponent<'b> + Send + Sync>(
        &self, entity: Entity, task: T
    ) {
        self.lazy.exec(move |world| {
            world.exec(move |(mut builder, mut task_storage): (TaskBuilder, WriteStorage<T>)| {
                builder.make_task_with_entity(entity, task, &mut task_storage);
            })
        })
    }

    /// Create a new task entity with the given `TaskComponent`. The task will not make progress
    /// until it is either finalized or the descendent of a finalized entity.
    pub fn make_task_lazy<'b, T: TaskComponent<'b> + Send + Sync>(&self, task: T) -> Entity {
        let entity = self.entities.create();
        self.make_task_with_entity_lazy(entity, task);

        entity
    }

    /// Create a new fork entity with no children.
    pub fn make_fork_lazy(&self) -> Entity {
        let entity = self
            .lazy
            .create_entity(&self.entities)
            .with(MultiEdge::default())
            .build();
        log::debug!("Created fork {:?}", entity);

        entity
    }

    /// Add `prong` as a child on the `MultiEdge` of `fork_entity`.
    pub fn add_prong_lazy(&self, fork_entity: Entity, prong: Entity) {
        self.lazy.exec(move |world| {
            world.exec(move |mut builder: TaskBuilder| {
                builder.add_prong(fork_entity, prong);
            })
        });
    }

    /// Creates a `SingleEdge` from `parent` to `child`. Creates a fork-join if `parent` is a fork.
    pub fn join_lazy(&self, parent: Entity, child: Entity) {
        self.lazy.exec(move |world| {
            world.exec(move |mut builder: TaskBuilder| {
                builder.join(parent, child);
            })
        });
    }

    /// Mark `entity` as final. This will make all of `entity`'s descendents visible to the
    /// `TaskManagerSystem`, allowing them to make progress. If `OnCompletion::Delete`, then
    /// `entity` and all of its descendents will be deleted when `entity` is complete (and hence the
    /// entire graph is complete). Otherwise, you need to clean up the entities your self by calling
    /// `delete_entity_and_descendents`. God help you if you leak an orphaned entity.
    pub fn finalize_lazy(&self, entity: Entity, on_completion: OnCompletion) {
        LazyBuilder {
            entity,
            lazy: &self.lazy,
        }
        .with(FinalTag { on_completion })
        .build();
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
    fn delete_descendents(&self, entity: Entity) {
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

    /// Returns `true` iff `entity` is complete.
    fn maintain_task_and_descendents(&self, entity: Entity) -> bool {
        let (is_unblocked, is_complete) = if let Some(progress) = self.progress.get(entity) {
            (progress.is_unblocked(), progress.is_complete())
        } else {
            // Missing progress means the task is complete and progress was already removed.
            return true;
        };

        if is_complete {
            log::debug!(
                "Noticed task {:?} is complete, removing TaskProgress",
                entity
            );
            return true;
        }

        // If `is_unblocked`, the children don't need maintenance, because we already verified they
        // are all complete.
        if is_unblocked {
            return false;
        }

        // Unblock the task if its child is complete.
        let mut child_complete = true;
        if let Some(SingleEdge { child }) = self.single_edges.get(entity).cloned() {
            child_complete = self.maintain_entity_and_descendents(child);
        }
        if child_complete {
            log::debug!("Unblocking task {:?}", entity);
            let progress = self
                .progress
                .get(entity)
                .expect("Blocked task must have progress");
            progress.unblock();
        }

        false
    }

    /// Returns `true` iff `entity` is complete.
    fn maintain_fork_and_descendents(
        &self,
        entity: Entity,
        multi_edge_children: &[Entity],
    ) -> bool {
        // We make sure that the SingleEdge child completes before any of the MultiEdge descendents
        // can start.
        let mut single_child_complete = true;
        if let Some(SingleEdge { child }) = self.single_edges.get(entity).cloned() {
            single_child_complete = self.maintain_entity_and_descendents(child);
        }
        let mut multi_children_complete = true;
        if single_child_complete {
            for child in multi_edge_children.iter() {
                multi_children_complete &= self.maintain_entity_and_descendents(*child);
            }
        }

        single_child_complete && multi_children_complete
    }

    /// Returns `true` iff `entity` is complete.
    pub(crate) fn maintain_entity_and_descendents(&self, entity: Entity) -> bool {
        // Only fork entities can have `MultiEdge`s, and they always do.
        if let Some(MultiEdge { children }) = self.multi_edges.get(entity).cloned() {
            self.maintain_fork_and_descendents(entity, &children)
        } else {
            self.maintain_task_and_descendents(entity)
        }
    }
}
