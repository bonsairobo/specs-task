use crate::{TaskComponent, TaskProgress};

use specs::prelude::*;
use std::marker::PhantomData;

/// The counterpart to an implementation `TaskComponent`. Runs tasks until completion.
///
/// See the tests for usage.
pub struct TaskRunnerSystem<T> {
    marker: PhantomData<T>,
}

impl<T> Default for TaskRunnerSystem<T> {
    fn default() -> Self {
        TaskRunnerSystem {
            marker: PhantomData,
        }
    }
}

impl<'a, T> System<'a> for TaskRunnerSystem<T>
where
    T: TaskComponent<'a>,
{
    // Note that we can only achieve task parallelism if `T::Data` does not have mutable overlap
    // with other `TaskComponent::Data`.
    type SystemData = (ReadStorage<'a, TaskProgress>, WriteStorage<'a, T>, T::Data);

    fn run(&mut self, (progress, mut tasks, mut task_data): Self::SystemData) {
        for (task_progress, task) in (&progress, &mut tasks).join() {
            if !task_progress.is_unblocked || task_progress.is_complete() {
                continue;
            }
            let is_complete = task.run(&mut task_data);
            if is_complete {
                task_progress.complete();
            }
        }
    }
}
