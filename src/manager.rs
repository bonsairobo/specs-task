use crate::{
    components::{FinalTag, MultiEdge, OnCompletion, SingleEdge, TaskProgress},
    data::TaskData,
};

use log::debug;
use specs::prelude::*;

/// Used for managing task entities after they've been created by the `TaskUser`.
#[doc(hidden)]
pub type TaskManager<'a> = TaskData<'a, WriteStorage<'a, TaskProgress>>;

impl TaskManager<'_> {
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
