use crate::components::{MultiEdge, TaskProgress};

use specs::{prelude::*, storage::GenericReadStorage};

#[derive(SystemData)]
pub struct TaskMonitor<'a> {
    progress: ReadStorage<'a, TaskProgress>,
    multi_edges: ReadStorage<'a, MultiEdge>,
}

impl<'a> TaskMonitor<'a> {
    /// Returns true iff the task was seen as complete on the last run of the `TaskManagerSystem`.
    pub fn task_is_complete(&self, task: Entity) -> bool {
        task_is_complete(&self.progress, task)
    }

    /// Tells you whether a fork or a task entity is complete.
    pub fn entity_is_complete(&self, entity: Entity) -> bool {
        entity_is_complete(&self.progress, &self.multi_edges, entity)
    }

    /// Returns the number of tasks that haven't yet completed.
    pub fn count_tasks_in_progress(&self) -> usize {
        (&self.progress).join().count()
    }
}

fn task_is_complete<S: GenericReadStorage<Component=TaskProgress>>(
    progress: &S, entity: Entity
) -> bool {
    if let Some(progress) = progress.get(entity) {
        progress.is_complete()
    } else {
        // Task entity may not have a TaskProgress component yet if it's constructed lazily.
        false
    }
}

/// Returns true iff all of `entity`'s children are complete.
fn fork_is_complete<S: GenericReadStorage<Component=TaskProgress>>(
    progress: &S,
    multi_edges: &ReadStorage<MultiEdge>,
    multi_children: &[Entity]
) -> bool {
    // We know that a fork's SingleEdge child is complete if any of the MultiEdge children are
    // complete.
    for child in multi_children.iter() {
        if !entity_is_complete(progress, multi_edges, *child) {
            return false;
        }
    }

    true
}

pub(crate) fn entity_is_complete<S: GenericReadStorage<Component=TaskProgress>>(
    progress: &S,
    multi_edges: &ReadStorage<MultiEdge>,
    entity: Entity,
) -> bool {
    // Only fork entities can have `MultiEdge`s. If the entity is being created lazily, we won't
    // know if it's a fork or task, but we won't consider it complete regardless.
    if let Some(MultiEdge { children }) = multi_edges.get(entity) {
        fork_is_complete(progress, multi_edges, &children)
    } else {
        task_is_complete(progress, entity)
    }
}
