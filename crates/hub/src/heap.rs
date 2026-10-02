//! 进程堆整理（第 4f 项 d 测量结果）：一次性的大块临时分配之后把空闲内存还给操作系统。

/// 把分配器中的空闲内存还给操作系统。
///
/// @why 启动时登记全部静态清单：解析后的清单（`serde_json::Value`）约为 Hub 保存的紧凑形式的 5 倍，转换后即释放，
/// 但 glibc 不主动归还堆中间的空闲页，1000 个清单时 RSS 停在约 260 MB（实际在用约 56 MB）。
/// @side-effect 只影响 RSS，不改变任何数据；Linux glibc 以外为空操作（其他平台的分配器自行归还）。
pub(crate) fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // @security 无指针参数；glibc 的 malloc_trim 线程安全，返回值只表示是否归还了内存。
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_is_safe_to_call_repeatedly() {
        let v: Vec<Vec<u8>> = (0..64).map(|_| vec![1u8; 64 * 1024]).collect();
        drop(v);
        super::release_free_memory();
        super::release_free_memory();
    }
}
