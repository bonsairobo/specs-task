use crate::{components::*, TaskData};

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
