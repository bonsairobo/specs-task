use crate::{
    components::{FinalTag, MultiEdge, OnCompletion, SingleEdge, TaskProgress},
    monitor::entity_is_complete,
};

use log::debug;
use specs::prelude::*;

/// Used for managing task entities after they've been created by the `TaskMaker`.
#[derive(SystemData)]
pub struct TaskManager<'a> {
    entities: Entities<'a>,
    progress: WriteStorage<'a, TaskProgress>,
    single_edges: ReadStorage<'a, SingleEdge>,
    multi_edges: ReadStorage<'a, MultiEdge>,
}

impl TaskManager<'_> {
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
        if entity_is_complete(&self.progress, &self.multi_edges, entity) {
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
