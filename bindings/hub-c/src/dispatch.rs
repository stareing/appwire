//! 回调分发：独立线程串行执行回调（不在调用方线程，也不在 tokio 工作线程）。
//!
//! - [`Dispatcher`]：分发线程与任务队列。
//! - [`UserData`]：调用方的 `user_data` + `free_user_data`。
//! - [`Slot`]：可替换的常驻回调（事件、审批、配对）。
//! - [`ResultCb`]：一次性结果回调，保证恰好调用一次（被丢弃而未完成时以兜底结果调用）。

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{JoinHandle, ThreadId};

use crate::ffi_util::into_raw_cstring;

pub type AmHubFreeFn = unsafe extern "C" fn(user_data: *mut c_void);
pub type AmHubResultFn = unsafe extern "C" fn(user_data: *mut c_void, result_json: *mut std::ffi::c_char);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

type Job = Box<dyn FnOnce() + Send + 'static>;

struct DispatcherInner {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    thread_id: ThreadId,
}

/// 分发线程。克隆共享同一线程。
#[derive(Clone)]
pub(crate) struct Dispatcher(Arc<DispatcherInner>);

impl Dispatcher {
    pub fn start() -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("am-hub-dispatch".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    // 回调中的 panic（理论上不会发生：C 回调不能 unwind）不能终止分发线程。
                    let _ = catch_unwind(AssertUnwindSafe(job));
                }
            })?;
        let thread_id = thread.thread().id();
        Ok(Self(Arc::new(DispatcherInner {
            tx: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(thread)),
            thread_id,
        })))
    }

    /// 排队执行；分发线程已关闭时原样退回任务。
    pub fn post(&self, job: Job) -> Result<(), Job> {
        match lock(&self.0.tx).as_ref() {
            Some(tx) => tx.send(job).map_err(|e| e.0),
            None => Err(job),
        }
    }

    pub fn is_dispatch_thread(&self) -> bool {
        std::thread::current().id() == self.0.thread_id
    }

    /// 关闭队列；不在分发线程上时等待已排队的任务执行完。
    pub fn close_and_join(&self) {
        lock(&self.0.tx).take();
        let handle = lock(&self.0.thread).take();
        if let Some(h) = handle
            && !self.is_dispatch_thread()
        {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// user_data
// ---------------------------------------------------------------------------

/// 可跨线程转交的裸指针。
#[derive(Clone, Copy)]
pub(crate) struct SendPtr(pub *mut c_void);

// SAFETY: 头文件约定回调在分发线程上执行、free_user_data 可能在任意线程调用，
// 即调用方传入 user_data 时已承诺它可以跨线程使用。本库只转交指针，从不解引用。
unsafe impl Send for SendPtr {}
// SAFETY: 同上。
unsafe impl Sync for SendPtr {}

/// 调用方的 `user_data`，丢弃时调用 `free_user_data`（若有）。
pub(crate) struct UserData {
    ptr: SendPtr,
    free: Option<AmHubFreeFn>,
}

impl UserData {
    pub fn new(ptr: *mut c_void, free: Option<AmHubFreeFn>) -> Self {
        Self {
            ptr: SendPtr(ptr),
            free,
        }
    }

    pub fn ptr(&self) -> *mut c_void {
        self.ptr.0
    }
}

impl Drop for UserData {
    fn drop(&mut self) {
        if let Some(free) = self.free.take() {
            // SAFETY: 调用方提供的释放函数，只调用一次。
            unsafe { free(self.ptr.0) };
        }
    }
}

/// 一个常驻回调：函数指针 + user_data。
pub(crate) struct CallbackEntry<F> {
    pub f: F,
    pub user_data: UserData,
}

/// 可替换的常驻回调。替换 / 清除时旧 user_data 在最后一个在途任务结束后释放。
pub(crate) struct Slot<F>(Mutex<Option<Arc<CallbackEntry<F>>>>);

impl<F> Slot<F> {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub fn get(&self) -> Option<Arc<CallbackEntry<F>>> {
        lock(&self.0).clone()
    }

    pub fn set(&self, entry: Option<CallbackEntry<F>>) {
        let old = std::mem::replace(&mut *lock(&self.0), entry.map(Arc::new));
        drop(old); // 在锁外释放（free_user_data 可能回调本库）
    }
}

// ---------------------------------------------------------------------------
// 一次性结果回调
// ---------------------------------------------------------------------------

/// 一次性结果回调：[`ResultCb::fire`] 或被丢弃（以兜底结果）时恰好调用一次。
pub(crate) struct ResultCb {
    f: Option<AmHubResultFn>,
    user_data: SendPtr,
    dispatcher: Dispatcher,
    /// 未调用 `fire` 就被丢弃时（Hub 停止、任务被中止）使用的结果。
    fallback: String,
}

impl ResultCb {
    pub fn new(f: AmHubResultFn, user_data: *mut c_void, dispatcher: Dispatcher, fallback: String) -> Self {
        Self {
            f: Some(f),
            user_data: SendPtr(user_data),
            dispatcher,
            fallback,
        }
    }

    /// 放弃回调（发起失败时，按约定不回调）。
    pub fn disarm(mut self) {
        self.f = None;
    }

    pub fn fire(mut self, json: String) {
        self.deliver(json);
    }

    fn deliver(&mut self, json: String) {
        let Some(f) = self.f.take() else { return };
        let ud = self.user_data;
        let job: Job = Box::new(move || {
            let ud = ud;
            // SAFETY: 调用方提供的回调；字符串所有权转移给回调方。
            unsafe { f(ud.0, into_raw_cstring(&json)) };
        });
        // 分发线程已关闭（极少见：释放过程中）时就地调用，保证恰好一次。
        if let Err(job) = self.dispatcher.post(job) {
            job();
        }
    }
}

impl Drop for ResultCb {
    fn drop(&mut self) {
        let json = std::mem::take(&mut self.fallback);
        self.deliver(json);
    }
}
