use crate::FlutterEngineWeakRef;
use flust_engine_sys::{FlutterEngineGetCurrentTime, FlutterTask};
use parking_lot::{Mutex, MutexGuard};
use priority_queue::PriorityQueue;
use std::cmp::{Ordering, Reverse};
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::thread;
use std::thread::ThreadId;
use std::time::{Duration, Instant};
use tracing::debug;

pub trait TaskRunnerHandler {
    fn wake(&self);
}

pub(crate) struct TaskRunnerInner {
    engine: FlutterEngineWeakRef,
    pub(crate) handler: Arc<dyn TaskRunnerHandler + Send + Sync>,
    thread_id: ThreadId,
    tasks: TaskQueue,
}

/// The tasks, earliest first: `PriorityQueue` hands out the greatest
/// priority first, and a task that is due earlier, or was posted earlier
/// for the same time, is the more urgent.
type TaskQueue = PriorityQueue<Task, Reverse<TaskPriority>>;

/// Takes the tasks that are due at `now` off the queue, in the order they
/// are to run.
fn take_due_tasks(tasks: &mut TaskQueue, now: Instant) -> Vec<Task> {
    let mut due = Vec::new();
    while let Some((_, Reverse(priority))) = tasks.peek() {
        if priority.time > now {
            break;
        }
        let (task, _) = tasks.pop().unwrap();
        due.push(task);
    }
    due
}

pub struct TaskRunner {
    pub(crate) inner: Arc<Mutex<TaskRunnerInner>>,
}

impl Clone for TaskRunner {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl TaskRunnerInner {
    pub(crate) fn runs_task_on_current_thread(&self) -> bool {
        thread::current().id() == self.thread_id
    }
}

impl TaskRunner {
    pub fn new(handler: Arc<dyn TaskRunnerHandler + Send + Sync>) -> Self {
        let thread_id = thread::current().id();
        debug!("task runner created on thread {:?}", thread_id);
        Self {
            inner: Arc::new(Mutex::new(TaskRunnerInner {
                engine: Default::default(),
                handler,
                thread_id,
                tasks: PriorityQueue::new(),
            })),
        }
    }

    pub(crate) fn init(&self, engine: FlutterEngineWeakRef) {
        let mut inner = self.inner.lock();
        inner.engine = engine;
    }

    pub fn execute_tasks(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut expired_tasks = Vec::new();

        let engine = {
            let mut inner = self.inner.lock();
            expired_tasks.append(&mut take_due_tasks(&mut inner.tasks, now));
            // make sure to unlock mutex before actually running the tasks as they may post another task
            inner.engine.upgrade().unwrap()
        };

        // run tasks
        for task in expired_tasks {
            engine.run_task(&task.task);
        }

        // next task time
        let inner = self.inner.lock();
        inner
            .tasks
            .peek()
            .map(|(_, Reverse(priority))| priority.time)
    }

    fn flutter_time_to_instant(target_time_nanos: u64) -> Instant {
        let current_time = unsafe { FlutterEngineGetCurrentTime() };
        let now = Instant::now();
        if current_time >= target_time_nanos {
            return now;
        }
        let nanos_timeout = target_time_nanos - current_time;
        now.checked_add(Duration::from_nanos(nanos_timeout))
            .unwrap()
    }

    pub(crate) fn post_task(
        guard: &mut MutexGuard<TaskRunnerInner>,
        task: FlutterTask,
        target_time_nanos: u64,
    ) {
        static GLOBAL_ORDER: AtomicU64 = AtomicU64::new(0);
        let task_priority = TaskPriority {
            time: Self::flutter_time_to_instant(target_time_nanos),
            order: GLOBAL_ORDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        };
        let task = Task { task };
        guard.tasks.push(task, Reverse(task_priority));

        let handler = guard.handler.clone();

        // make sure to unlock the mutex before posting an event because the event handler may read the queue
        MutexGuard::unlocked(guard, move || {
            handler.wake();
        });
    }

    pub(crate) fn runs_task_on_current_thread(&self) -> bool {
        self.inner.lock().runs_task_on_current_thread()
    }

    pub(crate) fn wake(&self) {
        let handler = { self.inner.lock().handler.clone() };
        handler.wake();
    }
}

#[derive(Eq, PartialEq)]
struct TaskPriority {
    order: u64,
    time: Instant,
}

struct Task {
    task: FlutterTask,
}

unsafe impl Send for Task {}

impl Ord for TaskPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.time.cmp(&other.time) {
            Ordering::Equal => self.order.cmp(&other.order),
            ord => ord,
        }
    }
}

impl PartialOrd for TaskPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.task.task == other.task.task && std::ptr::eq(self.task.runner, other.task.runner)
    }
}

impl Eq for Task {}

impl Hash for Task {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.task.task.hash(state);
        std::ptr::hash(self.task.runner, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64) -> Task {
        Task {
            task: FlutterTask {
                runner: std::ptr::null_mut(),
                task: id,
            },
        }
    }

    fn ids(tasks: Vec<Task>) -> Vec<u64> {
        tasks.into_iter().map(|task| task.task.task).collect()
    }

    #[test]
    fn due_tasks_run_in_the_order_they_were_posted() {
        let now = Instant::now();
        let mut tasks = TaskQueue::new();
        for order in 0..3 {
            tasks.push(task(order), Reverse(TaskPriority { order, time: now }));
        }
        assert_eq!(ids(take_due_tasks(&mut tasks, now)), vec![0, 1, 2]);
    }

    #[test]
    fn earlier_tasks_run_first() {
        let now = Instant::now();
        let mut tasks = TaskQueue::new();
        let at = |ms| now + Duration::from_millis(ms);
        tasks.push(
            task(0),
            Reverse(TaskPriority {
                order: 0,
                time: at(20),
            }),
        );
        tasks.push(
            task(1),
            Reverse(TaskPriority {
                order: 1,
                time: at(10),
            }),
        );
        assert_eq!(ids(take_due_tasks(&mut tasks, at(30))), vec![1, 0]);
    }

    #[test]
    fn a_task_for_later_does_not_hold_back_due_ones() {
        let now = Instant::now();
        let later = now + Duration::from_millis(500);
        let mut tasks = TaskQueue::new();
        tasks.push(
            task(0),
            Reverse(TaskPriority {
                order: 0,
                time: later,
            }),
        );
        tasks.push(
            task(1),
            Reverse(TaskPriority {
                order: 1,
                time: now,
            }),
        );
        assert_eq!(ids(take_due_tasks(&mut tasks, now)), vec![1]);
        assert_eq!(tasks.len(), 1);
    }
}
