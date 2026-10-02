//! launchd 连接器：在 Linux 上以真实 Unix 套接字扮演"launchd 持有的监听端 + App"（激活本身只能在 Mac 上验证）。

use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use app_mcp_protocol::naming::registration::{self, Activation, Registration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::super::Connector;
use super::super::registered::DirChange;
use super::*;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> Scratch {
    let d = std::env::temp_dir().join(format!("amcp-ld-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(d.join("apps")).expect("临时目录");
    std::fs::create_dir_all(d.join("run")).expect("套接字目录");
    std::fs::set_permissions(d.join("run"), std::fs::Permissions::from_mode(0o700)).expect("权限");
    Scratch(d)
}

impl Scratch {
    fn socket(&self, app_id: &str) -> PathBuf {
        self.0.join("run").join(names::socket_file_name(app_id))
    }

    fn write_reg(&self, app_id: &str, kind: &str, target: &str) {
        let reg = Registration {
            registration_version: registration::REGISTRATION_VERSION,
            app_id: app_id.into(),
            name: app_id.into(),
            source: "manual".into(),
            manifest: None,
            manifest_sha256: None,
            executable: None,
            activation: Activation { kind: kind.into(), target: target.into() },
        };
        let text = serde_json::to_string(&reg).expect("序列化");
        std::fs::write(self.0.join("apps").join(registration::file_name(app_id)), text).expect("写登记");
    }

    /// 以 launchd 方式登记 `app_id`（target = 套接字路径）。
    fn launchd_reg(&self, app_id: &str) -> PathBuf {
        let sock = self.socket(app_id);
        self.write_reg(app_id, kinds::LAUNCHD, &sock.display().to_string());
        sock
    }
}

/// 替身目录监视：测试经 `tx` 送变化；守卫被丢弃时置位。
#[derive(Default)]
struct FakeWatcher {
    tx: Mutex<Option<UnboundedSender<DirChange>>>,
    stopped: Arc<AtomicBool>,
}

struct StopFlag(Arc<AtomicBool>);
impl Drop for StopFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl DirWatcher for FakeWatcher {
    fn watch(&self, _dir: &Path) -> Result<Option<DirWatch>, ConnectorError> {
        let (tx, rx) = unbounded_channel();
        *self.tx.lock().unwrap() = Some(tx);
        Ok(Some(DirWatch { rx, guard: Box::new(StopFlag(self.stopped.clone())) }))
    }
}

fn connector(s: &Scratch) -> (LaunchdConnector, Arc<FakeWatcher>) {
    let w = Arc::new(FakeWatcher::default());
    (LaunchdConnector::with_watcher(s.0.join("apps"), w.clone()), w)
}

fn addr(s: &str) -> Address {
    Address::parse(s).expect("地址")
}

/// 扮演被 launchd 启动后的 App：接受一个连接，写 `first`，返回读到的内容。
fn fake_app(listener: UnixListener, first: &'static [u8]) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        if !first.is_empty() {
            s.write_all(first).await.expect("写");
        }
        let mut got = vec![0u8; 5];
        let n = s.read(&mut got).await.unwrap_or(0);
        got.truncate(n);
        got
    })
}

#[tokio::test]
async fn discover_lists_registrations_without_connecting() {
    let s = scratch();
    let sock = s.launchd_reg("shop");
    s.write_reg("other", kinds::EXEC, "");
    s.write_reg("rel", kinds::LAUNCHD, "relative.sock");
    std::fs::write(s.0.join("apps").join("bad.json"), "{").unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (c, _) = connector(&s);

    let mut found = c.discover().await.expect("发现");
    found.sort_by(|a, b| a.address.app_id.cmp(&b.address.app_id));
    let got: Vec<(&str, bool, bool, &str)> =
        found.iter().map(|n| (n.address.app_id.as_str(), n.activatable, n.running, n.detail.as_str())).collect();
    assert_eq!(
        got,
        [
            ("other", false, false, "dev.appmcp.App.other"),
            ("rel", false, false, "dev.appmcp.App.rel"),
            ("shop", true, false, "dev.appmcp.App.shop"),
        ]
    );
    // @invariant 发现不连接套接字（连接即激活，spec/naming.md 5.1）。
    assert_eq!(listener.accept().map_err(|e| e.kind()).err(), Some(std::io::ErrorKind::WouldBlock));
}

#[tokio::test]
async fn dial_connects_and_replays_upgrade() {
    let s = scratch();
    let sock = s.launchd_reg("shop");
    let app = fake_app(UnixListener::bind(&sock).unwrap(), b"GET /app HTTP/1.1\r\n");
    let (c, _) = connector(&s);
    let mut ch = c.dial(&addr("appmcp://shop"), Duration::from_secs(5)).await.expect("拨号");
    assert_eq!(ch.pid, Some(std::process::id()), "对端是本测试进程");
    let mut head = [0u8; 8];
    ch.stream.read_exact(&mut head).await.unwrap();
    assert_eq!(&head, b"GET /app", "升级请求原样回放");
    ch.stream.write_all(b"hello").await.unwrap();
    assert_eq!(app.await.unwrap(), b"hello", "写入到达 App");
}

#[tokio::test]
async fn dial_maps_refusal_timeout_and_close() {
    let s = scratch();
    let (c, _) = connector(&s);
    let cases: &[(&str, &'static [u8], &str)] = &[
        ("busy", b"CHANNEL_LIMIT\xEF\xBC\x9AApp \xE5\xB7\xB2\xE6\x9C\x89\xE8\xBF\x9E\xE6\x8E\xA5\n", codes::CHANNEL_LIMIT),
        ("stopped", b"ACTIVATION_DENIED: stopped\n", codes::ACTIVATION_DENIED),
    ];
    for (id, line, code) in cases {
        let sock = s.launchd_reg(id);
        let _app = fake_app(UnixListener::bind(&sock).unwrap(), line);
        let e = c.dial(&addr(&format!("appmcp://{id}")), Duration::from_secs(5)).await.expect_err("被拒");
        assert_eq!(e.code, *code, "{id}: {e}");
    }
    // App 起来后一直不发握手（或 launchd 没能启动它）：超时。
    let sock = s.launchd_reg("silent");
    let _l = UnixListener::bind(&sock).unwrap();
    let e = c.dial(&addr("appmcp://silent"), Duration::from_millis(200)).await.expect_err("超时");
    assert_eq!(e.code, codes::ACTIVATION_TIMEOUT, "{e}");
    // 未发送任何数据即关闭：激活被拒。
    let sock = s.launchd_reg("closer");
    let l = UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { drop(l.accept().await) });
    let e = c.dial(&addr("appmcp://closer"), Duration::from_secs(5)).await.expect_err("关闭");
    assert_eq!(e.code, codes::ACTIVATION_DENIED, "{e}");
}

#[tokio::test]
async fn dial_errors_before_connecting() {
    let s = scratch();
    let (c, _) = connector(&s);
    let dial = |a: &'static str| {
        let c = &c;
        async move { c.dial(&addr(a), Duration::from_secs(1)).await.expect_err(a).code }
    };
    assert_eq!(dial("appmcp://nobody").await, codes::NAME_NOT_FOUND, "没有登记");
    s.launchd_reg("shop");
    assert_eq!(dial("appmcp://shop/w2").await, codes::NAME_NOT_FOUND, "实例不登记");
    assert_eq!(dial("appmcp://shop").await, codes::NAME_NOT_FOUND, "套接字不存在：作业未载入");
    // 套接字文件还在但无人监听（作业被卸下后的残留）。
    drop(std::os::unix::net::UnixListener::bind(s.socket("shop")).unwrap());
    assert_eq!(dial("appmcp://shop").await, codes::ACTIVATION_DENIED, "无人监听");
    s.write_reg("exe", kinds::EXEC, "");
    assert_eq!(dial("appmcp://exe").await, codes::ACTIVATION_DENIED, "不是 launchd 激活");
    let long = format!("/{}/x.sock", "d".repeat(200));
    s.write_reg("long", kinds::LAUNCHD, &long);
    assert_eq!(dial("appmcp://long").await, codes::NAME_NOT_FOUND, "路径过长");
    // 组可写的套接字目录：不连接。
    let sock = s.launchd_reg("open");
    let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    l.set_nonblocking(true).unwrap();
    std::fs::set_permissions(s.0.join("run"), std::fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(dial("appmcp://open").await, codes::PEER_IDENTITY_MISMATCH, "目录组可写");
    assert_eq!(l.accept().map_err(|e| e.kind()).err(), Some(std::io::ErrorKind::WouldBlock), "未连接");
}

#[test]
fn identity_rules() {
    assert_eq!(names::socket_dir_issue(501, 0o40700, 501), None);
    assert_eq!(names::socket_dir_issue(501, 0o40755, 501), None);
    assert!(names::socket_dir_issue(0, 0o40700, 501).is_some(), "属主不是当前用户");
    assert!(names::socket_dir_issue(501, 0o40770, 501).is_some(), "组可写");
    assert!(names::socket_dir_issue(501, 0o40702, 501).is_some(), "其他用户可写");
    assert!(trusted_peer(501, 501));
    assert!(trusted_peer(0, 501), "launchd（root）持有的监听端");
    assert!(!trusted_peer(502, 501));
}

#[tokio::test]
async fn watch_turns_rescans_into_events() {
    use futures::StreamExt;
    let s = scratch();
    s.launchd_reg("shop");
    let (c, w) = connector(&s);
    let mut events = c.watch().await.expect("订阅");
    assert_eq!(c.discover().await.expect("发现").len(), 1);
    let tx = w.tx.lock().unwrap().clone().expect("监视已建立");

    s.launchd_reg("linky");
    std::fs::remove_file(s.0.join("apps").join("shop.json")).unwrap();
    tx.send(DirChange::Rescan).unwrap();
    let mut got = vec![events.next().await, events.next().await];
    got.sort_by_key(|e| format!("{e:?}"));
    assert!(matches!(&got[0], Some(NameEvent::Installed(n)) if n.address.app_id == "linky" && n.activatable), "{got:?}");
    assert_eq!(got[1], Some(NameEvent::Removed(addr("appmcp://shop"))));
    assert!(!w.stopped.load(Ordering::SeqCst));
    drop(events);
    assert!(w.stopped.load(Ordering::SeqCst), "事件流被丢弃即停止监视");
}
