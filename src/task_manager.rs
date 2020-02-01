use crate::{TaskComponent, TaskProgress};

use log::debug;
use specs::{prelude::*, storage::StorageEntry};
use std::{error, fmt};

/// This error means the entity provided to one of the APIs did not have the expected components.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnexpectedEntity {
    ExpectedTaskEntity(Entity),
    ExpectedForkEntity(Entity),
}

impl fmt::Display for UnexpectedEntity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ExpectedTaskEntity(e) => write!(f, "Expected {:?} to be a task", e),
            Self::ExpectedForkEntity(e) => write!(f, "Expected {:?} to be a fork", e),
        }
    }
}

impl error::Error for UnexpectedEntity {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

/// This error means that you tried to `join` an entity that was already `joined` (has a
/// `SingleEdge`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlreadyJoined {
    pub parent: Entity,
    pub already_child: Entity,
    pub new_child: Entity,
}

impl fmt::Display for AlreadyJoined {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Tried to join {:?} --> {:?}, but the SingleEdge {:?} --> {:?} already exists",
            self.parent, self.already_child, self.parent, self.new_child,
        )
    }
}

impl error::Error for AlreadyJoined {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct SingleEdge {
    child: Entity,
}

impl Component for SingleEdge {
    type Storage = VecStorage<Self>;
}

#[doc(hidden)]
#[derive(Clone, Default)]
pub struct MultiEdge {
    children: Vec<Entity>,
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
    on_completion: OnCompletion,
}

impl Component for FinalTag {
    type Storage = VecStorage<Self>;
}

/// The main object for users of this module. Used for creating and connecting tasks.
#[derive(SystemData)]
pub struct TaskManager<'a> {
    entities: Entities<'a>,
    progress: WriteStorage<'a, TaskProgress>,
    single_edges: WriteStorage<'a, SingleEdge>,
    multi_edges: WriteStorage<'a, MultiEdge>,
    finalized: WriteStorage<'a, FinalTag>,
}

impl TaskManager<'_> {
    /// Like `make_task`, but use `entity` for tracking the task components. This can make it easier
    /// to manage tasks coupled with a specific entity (rather than storing a separate task entity
    /// in a component).
    pub fn make_task_with_entity<'a, T: TaskComponent<'a>>(
        &mut self,
        entity: Entity,
        task: T,
        tasks: &mut WriteStorage<T>,
    ) {
        self.progress.insert(entity, TaskProgress::default()).unwrap();
        tasks.insert(entity, task).unwrap();
        debug!("Created task {:?}", entity);
    }

    /// Create a new task entity with the given `TaskComponent`. The task will not make progress
    /// until it is either finalized or the descendent of a finalized entity.
    pub fn make_task<'a, T: TaskComponent<'a>>(
        &mut self,
        task: T,
        tasks: &mut WriteStorage<T>,
    ) -> Entity {
        let entity = self.entities.create();
        self.make_task_with_entity(entity, task, tasks);

        entity
    }

    /// Same as `make_task_with_entity`, but also finalizes the task.
    pub fn make_final_task_with_entity<'a, T: TaskComponent<'a>>(
        &mut self,
        entity: Entity,
        task: T,
        tasks: &mut WriteStorage<T>,
        on_completion: OnCompletion,
    ) -> Entity {
        self.make_task_with_entity(entity, task, tasks);
        self.finalize(entity, on_completion);

        entity
    }

    /// Same as `make_task`, but also finalizes the task.
    pub fn make_final_task<'a, T: TaskComponent<'a>>(
        &mut self,
        task: T,
        tasks: &mut WriteStorage<T>,
        on_completion: OnCompletion,
    ) -> Entity {
        let task_entity = self.make_task(task, tasks);
        self.finalize(task_entity, on_completion);

        task_entity
    }

    /// Create a new fork entity with no children.
    pub fn make_fork(&mut self) -> Entity {
        let entity = self.entities.create();
        self.multi_edges.insert(entity, MultiEdge::default()).unwrap();
        debug!("Created fork {:?}", entity);

        entity
    }

    /// Add `prong` as a child on the `MultiEdge` of `fork_entity`.
    pub fn add_prong(
        &mut self,
        fork_entity: Entity,
        prong: Entity,
    ) -> Result<(), UnexpectedEntity> {
        let multi_edge = self
            .multi_edges
            .get_mut(fork_entity)
            .ok_or(UnexpectedEntity::ExpectedForkEntity(fork_entity))?;
        multi_edge.add_child(prong);

        Ok(())
    }

    /// Creates a `SingleEdge` from `parent` to `child`. Creates a fork-join if `parent` is a fork.
    pub fn join(&mut self, parent: Entity, child: Entity) -> Result<(), AlreadyJoined> {
        let entry = self.single_edges.entry(parent).unwrap();

        match entry {
            StorageEntry::Occupied(e) => Err(AlreadyJoined {
                parent,
                already_child: e.get().child,
                new_child: child,
            }),
            StorageEntry::Vacant(e) => {
                e.insert(SingleEdge { child });

                Ok(())
            }
        }
    }

    /// Mark `entity` as final. This will make all of `entity`'s descendents visible to the
    /// `TaskManagerSystem`, allowing them to make progress. If `OnCompletion::Delete`, then
    /// `entity` and all of its descendents will be deleted when `entity` is complete (and hence the
    /// entire graph is complete). Otherwise, you need to clean up the entities your self by calling
    /// `delete_entity_and_descendents`. God help you if you leak an orphaned entity.
    pub fn finalize(&mut self, entity: Entity, on_completion: OnCompletion) {
        self.finalized.insert(
            entity,
            FinalTag {
                on_completion,
            },
        ).unwrap();
    }

    /// Returns true iff the task was seen as complete on the last run of the `TaskManagerSystem`.
    ///
    /// WARNING: assumes that this entity was at one point a task, and it can't tell otherwise.
    pub fn task_is_complete(&self, entity: Entity) -> bool {
        !self.progress.contains(entity)
    }

    /// Returns true iff all of `entity`'s children are complete.
    fn fork_is_complete(&self, entity: Entity, multi_children: &[Entity]) -> bool {
        if let Some(SingleEdge { child }) = self.single_edges.get(entity) {
            if !self.entity_is_complete(*child) {
                return false;
            }
        }
        for child in multi_children.iter() {
            if !self.entity_is_complete(*child) {
                return false;
            }
        }

        true
    }

    /// Tells you whether a fork or a task entity is complete.
    ///
    /// WARNING: assumes that this entity was at one point a task or a fork, and it can't tell
    /// otherwise.
    pub fn entity_is_complete(&self, entity: Entity) -> bool {
        // Only fork entities can have `MultiEdge`s, and they always do.
        if let Some(MultiEdge { children }) = self.multi_edges.get(entity) {
            self.fork_is_complete(entity, &children)
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
            // Task will no longer be considered by the `TaskRunnerSystem`.
            self.progress.remove(entity);
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
///   - removes `TaskProgress` components from completed tasks
///   - deletes task graphs with `OnCompletion::Delete`
///   - removes `FinalTag` components from completed entities
pub struct TaskManagerSystem;

impl<'a> System<'a> for TaskManagerSystem {
    type SystemData = TaskManager<'a>;

    fn run(&mut self, mut task_man: Self::SystemData) {
        let final_ents: Vec<(Entity, FinalTag)> = (&task_man.entities, &task_man.finalized)
            .join()
            .map(|(e, f)| (e, *f))
            .collect();
        for (
            entity,
            FinalTag {
                on_completion,
            },
        ) in final_ents.into_iter()
        {
            let final_complete = task_man.maintain_entity_and_descendents(entity);
            if final_complete {
                match on_completion {
                    OnCompletion::Delete => { task_man.delete_entity_and_descendents(entity); }
                    OnCompletion::DeleteDescendents => { task_man.delete_descendents(entity); }
                    OnCompletion::None => {
                        debug!("Removing FinalTag from {:?}", entity);
                        task_man.finalized.remove(entity);
                    }
                }
            }
        }
    }
}
