//! 句柄内部状态（`CallInner`、`ReadInner`、`NavigateInner`、`HoldInner`）的实现。

use super::*;

impl std::fmt::Debug for CallInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallInner")
            .field("call_id", &self.call_id)
            .field("tool_name", &self.tool_name)
            .finish_non_exhaustive()
    }
}

impl CallInner {
    pub(crate) fn lock(&self) -> MutexGuard<'_, CallState> {
        lock_ignore_poison(&self.state)
    }

    /// 标记已取消，返回需要通知的监听器。
    pub(crate) fn mark_cancelled(&self, reason: CancelReason) -> Option<Arc<dyn CancelListener>> {
        let mut st = self.lock();
        if st.cancelled.is_some() {
            return None;
        }
        st.cancelled = Some(reason);
        st.listener.take()
    }

    pub(crate) fn finish(&self, outcome: Result<CallOutput, ToolError>) -> Result<(), NativeError> {
        {
            let mut st = self.lock();
            if st.finished || st.cancelled.is_some() {
                return Err(NativeError::AlreadyCompleted);
            }
            st.finished = true;
        }
        let mut core = self.shared.lock();
        core.calls.remove(&self.call_id);
        let result = core.client.complete_call(&self.call_id, outcome, now_ms());
        drop(core);
        self.shared.wake();
        result.map_err(core_error)
    }
}

impl std::fmt::Debug for ReadInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadInner")
            .field("read", &self.read)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ReadInner {
    pub(crate) fn finish(&self, outcome: Result<Value, ToolError>) -> Result<(), NativeError> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Err(NativeError::AlreadyCompleted);
        }
        let result = self.shared.lock().client.complete_read(self.read, outcome);
        self.shared.wake();
        result.map_err(core_error)
    }
}

impl std::fmt::Debug for NavigateInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavigateInner")
            .field("navigate", &self.navigate)
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

impl NavigateInner {
    pub(crate) fn finish(&self, outcome: Result<(), ToolError>) -> Result<(), NativeError> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Err(NativeError::AlreadyCompleted);
        }
        let result = self.shared.lock().client.complete_navigate(self.navigate, outcome);
        self.shared.wake();
        result.map_err(core_error)
    }
}

impl HoldInner {
    pub(crate) fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.lock().client.release_hold(self.id, now_ms());
        self.shared.wake();
    }
}

impl Drop for HoldInner {
    fn drop(&mut self) {
        self.release();
    }
}
