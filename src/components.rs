use specs::prelude::*;
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
    pub(crate) fn add_child(&mut self, entity: Entity) {
        self.children.push(entity);
    }
}

impl Component for MultiEdge {
    type Storage = VecStorage<Self>;
}

/// What to do to a final task and its descendents when it they complete.
/// WARNING: If you specify `Delete`, then you will not be able to poll for completion, since a
/// non-existent entity is assumed to be "incomplete."
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum OnCompletion {
    None,
    Delete,
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
