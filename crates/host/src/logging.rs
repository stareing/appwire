//! 日志：始终写 stderr（stdout 专用于 MCP）；常驻模式另写 `<home>/logs/app-mcp-host.log`，按大小轮转。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer as _};

/// 日志文件名。
pub const LOG_FILE: &str = "app-mcp-host.log";

/// 按大小轮转的日志文件：写满 `max_bytes` 后 `x.log` → `x.log.1` → … → `x.log.<keep>`（最旧的删除）。
#[derive(Clone)]
pub struct RotatingFile {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    pub fn open(path: PathBuf, max_bytes: u64, keep: usize) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                path,
                max_bytes: max_bytes.max(1024),
                keep,
                file: Some(file),
                size,
            })),
        })
    }
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        if self.keep == 0 {
            let _ = std::fs::remove_file(&self.path);
        } else {
            let _ = std::fs::remove_file(numbered(&self.path, self.keep));
            for n in (1..self.keep).rev() {
                let from = numbered(&self.path, n);
                if from.exists() {
                    std::fs::rename(&from, numbered(&self.path, n + 1))?;
                }
            }
            std::fs::rename(&self.path, numbered(&self.path, 1))?;
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.size = 0;
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            // 轮转失败（如 Windows 上文件被占用）时继续写原文件，不丢日志。
            if self.rotate().is_err() && self.file.is_none() {
                self.file = Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)?,
                );
            }
        }
        let file = match self.file.as_mut() {
            Some(f) => f,
            None => return Ok(buf.len()),
        };
        file.write_all(buf)?;
        self.size += buf.len() as u64;
        Ok(buf.len())
    }
}

/// 一次日志事件的写入句柄。
pub struct RotatingWriter(Arc<Mutex<Inner>>);

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.lock() {
            Ok(mut inner) => inner.write(buf),
            Err(_) => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RotatingFile {
    type Writer = RotatingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RotatingWriter(self.inner.clone())
    }
}

/// 日志设置。
pub struct LogOptions<'a> {
    pub level: &'a str,
    /// `(目录, 单文件上限, 保留数)`；`None` 只写 stderr。
    pub file: Option<(PathBuf, u64, usize)>,
}

/// 初始化全局日志。返回日志文件路径（若启用）。
pub fn init(options: LogOptions<'_>) -> anyhow::Result<Option<PathBuf>> {
    let filter = || {
        EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(options.level))
            .context("无效的日志级别")
    };
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_ansi(false)
        .with_filter(filter()?);
    let (file_layer, path) = match &options.file {
        Some((dir, max_bytes, keep)) => {
            let path = dir.join(LOG_FILE);
            let writer = RotatingFile::open(path.clone(), *max_bytes, *keep)
                .with_context(|| format!("打开日志文件 {} 失败", path.display()))?;
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_filter(filter()?);
            (Some(layer), Some(path))
        }
        None => (None, None),
    };
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .context("日志已初始化")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_by_size() {
        let dir = std::env::temp_dir().join(format!("app-mcp-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(LOG_FILE);
        let f = RotatingFile::open(path.clone(), 1024, 2).unwrap();
        let line = [b'x'; 300];
        for _ in 0..20 {
            f.make_writer().write_all(&line).unwrap();
        }
        let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        assert!(len(&path) <= 1024 && len(&path) > 0);
        assert!(len(&numbered(&path, 1)) > 0);
        assert!(len(&numbered(&path, 2)) > 0);
        assert!(!numbered(&path, 3).exists(), "只保留 2 个历史文件");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
