# Fork-join multitasking for SPECS ECS

Instead of hand-rolling state machines to sequence the effects of various ECS systems, spawn
tasks as entities and declare explicit temporal dependencies between them.

## Code Examples

### Making task graphs

```rust
fn make_static_task_graph(user: &TaskUser) {
    // Any component that implements TaskComponent can be spawned.
    let task_graph: TaskGraph = seq!(
        @TaskFoo("hello"),
        fork!(
            @TaskBar { value: 1 },
            @TaskBar { value: 2 },
            @TaskBar { value: 3 }
        ),
        @TaskZing("goodbye")
    );
    task_graph.assemble(user, OnCompletion::Delete);
}

fn make_dynamic_task_graph(user: &TaskUser) {
    let first = task!(@TaskFoo("hello"));
    let mut middle = empty_graph!();
    for i in 0..10 {
        middle = fork!(middle, @TaskBar { value: i });
    }
    let last = task!(@TaskZing("goodbye"));
    let task_graph: TaskGraph = seq!(first, middle, last);
    task_graph.assemble(user, OnCompletion::Delete);
}
```

### Building a dispatcher with a `TaskRunnerSystem`

```rust
#[derive(Clone, Debug)]
struct PushValue {
    value: usize,
}

impl Component for PushValue {
    type Storage = VecStorage<Self>;
}

impl<'a> TaskComponent<'a> for PushValue {
    type Data = Write<'a, Vec<usize>>;

    fn run(&mut self, data: &mut Self::Data) -> bool {
        data.push(self.value);

        true
    }
}

fn make_dispatcher() -> Dispatcher {
    DispatcherBuilder::new()
        .with(
            TaskRunnerSystem::<PushValue>::default(),
            "push_value",
            &[],
        )
        .with(
            TaskManagerSystem,
            "task_manager",
            &[],
        )
        .build()
}
```
