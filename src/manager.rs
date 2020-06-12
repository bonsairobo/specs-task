use crate::{
    components::{FinalTag, OnCompletion},
    user::TaskUser,
};

use log::debug;
use specs::prelude::*;

/// Traverses all descendents of all finalized entities and unblocks them if possible.
///
/// Also does some garbage collection:
///   - deletes task graphs with `OnCompletion::Delete`
///   - removes `FinalTag` components from completed entities
pub struct TaskManagerSystem;

impl<'a> System<'a> for TaskManagerSystem {
    type SystemData = (TaskUser<'a>, WriteStorage<'a, FinalTag>);

    fn run(&mut self, (task_user, mut finalized): Self::SystemData) {
        let final_ents: Vec<(Entity, FinalTag)> = (&task_user.entities, &finalized)
            .join()
            .map(|(e, f)| (e, *f))
            .collect();
        for (entity, FinalTag { on_completion }) in final_ents.into_iter() {
            let final_complete = task_user.maintain_entity_and_descendents(entity);
            if final_complete {
                match on_completion {
                    OnCompletion::Delete => {
                        task_user.delete_entity_and_descendents(entity);
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
