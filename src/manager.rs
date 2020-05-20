use crate::components::{FinalTag, MultiEdge, OnCompletion, SingleEdge, TaskProgress};

use log::debug;
use specs::prelude::*;

/// The main object for users of this module. Used for creating and connecting tasks.
#[derive(SystemData)]
pub struct TaskManager<'a> {
    entities: Entities<'a>,
    progress: WriteStorage<'a, TaskProgress>,
    single_edges: ReadStorage<'a, SingleEdge>,
    multi_edges: ReadStorage<'a, MultiEdge>,
}

impl TaskManager<'_> {
    /// Returns true iff the task was seen as complete on the last run of the `TaskManagerSystem`.
    pub fn task_is_complete(&self, entity: Entity) -> bool {
        if let Some(progress) = self.progress.get(entity) {
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
    pub fn count_tasks_in_progress(&self) -> usize {
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
        debug!("Deleting {:?}", entity);
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
    fn maintain_task_and_descendents(&mut self, entity: Entity) -> bool {
        let (is_unblocked, is_complete) = if let Some(progress) = self.progress.get(entity) {
            (progress.is_unblocked, progress.is_complete())
        } else {
            // Missing progress means the task is complete and progress was already removed.
            return true;
        };

        if is_complete {
            debug!(
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
            debug!("Unblocking task {:?}", entity);
            let progress = self
                .progress
                .get_mut(entity)
                .expect("Blocked task must have progress");
            progress.unblock();
        }

        false
    }

    /// Returns `true` iff `entity` is complete.
    fn maintain_fork_and_descendents(
        &mut self,
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
    fn maintain_entity_and_descendents(&mut self, entity: Entity) -> bool {
        // Only fork entities can have `MultiEdge`s, and they always do.
        if let Some(MultiEdge { children }) = self.multi_edges.get(entity).cloned() {
            self.maintain_fork_and_descendents(entity, &children)
        } else {
            self.maintain_task_and_descendents(entity)
        }
    }
}

/// Traverses all descendents of all finalized entities and unblocks them if possible.
///
/// Also does some garbage collection:
///   - deletes task graphs with `OnCompletion::Delete`
///   - removes `FinalTag` components from completed entities
pub struct TaskManagerSystem;

impl<'a> System<'a> for TaskManagerSystem {
    type SystemData = (TaskManager<'a>, WriteStorage<'a, FinalTag>);

    fn run(&mut self, (mut task_man, mut finalized): Self::SystemData) {
        let final_ents: Vec<(Entity, FinalTag)> = (&task_man.entities, &finalized)
            .join()
            .map(|(e, f)| (e, *f))
            .collect();
        for (entity, FinalTag { on_completion }) in final_ents.into_iter() {
            let final_complete = task_man.maintain_entity_and_descendents(entity);
            if final_complete {
                match on_completion {
                    OnCompletion::Delete => {
                        task_man.delete_entity_and_descendents(entity);
                    }
                    OnCompletion::DeleteDescendents => {
                        task_man.delete_descendents(entity);
                    }
                    OnCompletion::None => {
                        debug!("Removing FinalTag from {:?}", entity);
                        finalized.remove(entity);
                    }
                }
            }
        }
    }
}
