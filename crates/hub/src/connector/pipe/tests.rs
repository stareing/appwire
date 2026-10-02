//! 连接器的平台无关逻辑（各平台都运行）：登记目录发现、拨号的激活与退避、身份核对、拒绝识别、目录事件。
//! 平台操作用替身 [`Fake`]；Windows 上的真实管道 / 进程由 `crates/hub/tests/naming_pipe.rs` 覆盖。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::*;
use crate::connector::registered::lock;
use app_mcp_protocol::naming::registration;
use crate::connector::Connector;

const SID: &str = "S-1-5-21-1-2-3-1001";
const HELLO: &[u8] = b"GET /app HTTP/1.1\r\nHost: localhost\r\n\r\n";

enum Step {
    Absent,
    Busy,
    /// App 侧写出 `writes` 后：`keep` 为真时保持连接，否则关闭。
    Pipe { writes: Vec<u8>, keep: bool, image: Option<String> },
}

#[derive(Default)]
struct Fake {
    steps: Mutex<VecDeque<Step>>,
    opens: AtomicUsize,
    activations: AtomicUsize,
    /// 激活后的进程以失败退出。
    launch_fails: AtomicBool,
    activate_error: Mutex<Option<ConnectorError>>,
    watch_tx: Mutex<Option<UnboundedSender<DirChange>>>,
    watch_stopped: Arc<AtomicBool>,
    /// App 侧保持打开的流（`keep`）。
    kept: Mutex<Vec<tokio::io::DuplexStream>>,
}

impl Fake {
    fn with(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self { steps: Mutex::new(steps.into()), ..Default::default() })
    }
}

struct FailedLaunch(bool);
impl Launched for FailedLaunch {
    fn failure(&mut self) -> Option<String> {
        self.0.then(|| "退出码 1".to_owned())
    }
}

struct StopFlag(Arc<AtomicBool>);
impl Drop for StopFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl PipeSystem for Fake {
    fn open(&self, _name: &str) -> Result<PipeOpen, ConnectorError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        match lock(&self.steps).pop_front().unwrap_or(Step::Absent) {
            Step::Absent => Ok(PipeOpen::Absent),
            Step::Busy => Ok(PipeOpen::Busy),
            Step::Pipe { writes, keep, image } => {
                let (hub, mut app) = tokio::io::duplex(4096);
                if keep {
                    let mut app_side = app;
                    let w = writes.clone();
                    // 写入在缓冲内立即完成（duplex 容量足够），之后保存以保持连接。
                    futures::executor::block_on(app_side.write_all(&w)).expect("写入");
                    lock(&self.kept).push(app_side);
                } else {
                    tokio::spawn(async move {
                        let _ = app.write_all(&writes).await;
                        drop(app);
                    });
                }
                Ok(PipeOpen::Opened(OpenedPipe { stream: Box::new(hub), server_pid: Some(42), server_image: image }))
            }
        }
    }

    async fn activate(&self, _reg: &Registration) -> Result<Box<dyn Launched>, ConnectorError> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = lock(&self.activate_error).take() {
            return Err(e);
        }
        Ok(Box::new(FailedLaunch(self.launch_fails.load(Ordering::SeqCst))))
    }

    fn watch(&self, _dir: &Path) -> Result<Option<DirWatch>, ConnectorError> {
        let (tx, rx) = unbounded_channel();
        *lock(&self.watch_tx) = Some(tx);
        Ok(Some(DirWatch { rx, guard: Box::new(StopFlag(self.watch_stopped.clone())) }))
    }
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> Scratch {
    let d = std::env::temp_dir().join(format!("app-mcp-pipe-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(d.join("apps")).expect("临时目录");
    Scratch(d)
}

fn exe(dir: &Path, name: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, b"x").expect("写程序");
    p.display().to_string()
}

fn write_reg(dir: &Path, app_id: &str, kind: &str, target: &str, executable: Option<&str>) {
    let reg = Registration {
        registration_version: registration::REGISTRATION_VERSION,
        app_id: app_id.into(),
        name: app_id.into(),
        source: "manual".into(),
        manifest: None,
        manifest_sha256: None,
        executable: executable.map(str::to_owned),
        activation: registration::Activation { kind: kind.into(), target: target.into() },
    };
    let text = serde_json::to_string(&reg).expect("序列化");
    std::fs::write(dir.join("apps").join(registration::file_name(app_id)), text).expect("写登记");
}

fn connector(dir: &Path, system: Arc<Fake>) -> PipeConnector {
    PipeConnector::with_system(dir.join("apps"), SID.into(), system)
}

fn addr(s: &str) -> Address {
    Address::parse(s).expect("地址")
}

#[tokio::test]
async fn discover_reads_only_valid_registrations() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    write_reg(&s.0, "linky", kinds::URI, "linky-app", None);
    write_reg(&s.0, "packed", kinds::AUMID, "Co.Packed_8wekyb3d8bbwe!App", None);
    write_reg(&s.0, "manual", kinds::NONE, "", None);
    write_reg(&s.0, "gone", kinds::EXEC, "", Some(&s.0.join("missing.exe").display().to_string()));
    write_reg(&s.0, "apps", kinds::EXEC, "", Some(&shop));
    let apps = s.0.join("apps");
    std::fs::write(apps.join("broken.json"), "{").expect("写");
    std::fs::write(apps.join("other.json"), std::fs::read(apps.join("shop.json")).expect("读")).expect("写");
    std::fs::write(apps.join("shop.tmp-1"), "x").expect("写");
    let fake = Fake::with(vec![]);
    let c = connector(&s.0, fake.clone());
    let found = c.discover().await.expect("发现");
    let summary: Vec<(String, bool, bool)> =
        found.iter().map(|n| (n.address.app_id.clone(), n.activatable, n.running)).collect();
    assert_eq!(
        summary,
        vec![
            ("linky".into(), true, false),
            ("manual".into(), false, false),
            ("packed".into(), true, false),
            ("shop".into(), true, false),
        ]
    );
    let shop_name = found.iter().find(|n| n.address.app_id == "shop").map(|n| n.detail.clone());
    assert_eq!(shop_name.as_deref(), Some(r"\\.\pipe\appmcp-S-1-5-21-1-2-3-1001-shop"));
    assert_eq!(fake.opens.load(Ordering::SeqCst) + fake.activations.load(Ordering::SeqCst), 0, "发现不得打开管道或激活");
    // 目录不存在：空结果，不报错。
    let none = PipeConnector::with_system(s.0.join("nope"), SID.into(), Fake::with(vec![]));
    assert!(none.discover().await.expect("发现").is_empty());
}

#[tokio::test]
async fn dial_activates_once_then_waits_for_pipe() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let fake = Fake::with(vec![
        Step::Absent,
        Step::Absent,
        Step::Busy,
        Step::Pipe { writes: HELLO.to_vec(), keep: true, image: Some(format!(r"\\?\{}", shop.to_uppercase())) },
    ]);
    let c = connector(&s.0, fake.clone());
    let mut ch = c.dial(&addr("appmcp://shop"), Duration::from_secs(10)).await.expect("拨号");
    assert_eq!(fake.activations.load(Ordering::SeqCst), 1, "只激活一次");
    assert_eq!(fake.opens.load(Ordering::SeqCst), 4);
    assert_eq!(ch.pid, Some(42));
    // 已读的升级请求原样回放给 App 连接服务。
    let mut got = vec![0u8; HELLO.len()];
    ch.stream.read_exact(&mut got).await.expect("回放");
    assert_eq!(got, HELLO);
}

#[tokio::test]
async fn dial_reports_timeout_when_pipe_never_appears() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let fake = Fake::with(vec![]);
    let c = connector(&s.0, fake.clone());
    let started = Instant::now();
    let window = Duration::from_millis(1600);
    let err = c.dial(&addr("appmcp://shop"), window).await.expect_err("应超时");
    assert_eq!(err.code, codes::ACTIVATION_TIMEOUT, "{err}");
    assert_eq!(fake.activations.load(Ordering::SeqCst), 1);
    assert!(Instant::now() - started <= window, "不超过激活窗口");
    // 退避：50、100、200、400、800 ms（下一次 1 s 会越过窗口，提前结束），共 6 次打开（不是忙等）。
    let opens = fake.opens.load(Ordering::SeqCst);
    assert!((4..=8).contains(&opens), "打开次数 {opens}");
}

#[tokio::test]
async fn dial_fails_fast_when_launch_exits_with_error() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let fake = Fake::with(vec![]);
    fake.launch_fails.store(true, Ordering::SeqCst);
    let c = connector(&s.0, fake.clone());
    let err = c.dial(&addr("appmcp://shop"), Duration::from_secs(10)).await.expect_err("应失败");
    assert_eq!(err.code, codes::ACTIVATION_DENIED, "{err}");
    assert!(fake.opens.load(Ordering::SeqCst) <= 2, "不等到超时");
}

#[tokio::test]
async fn dial_maps_app_refusal_and_early_close() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let refusal = format!("{}\n", names::refusal_line(codes::CHANNEL_LIMIT, "App 已有连接"));
    let cases = [
        (refusal.into_bytes(), codes::CHANNEL_LIMIT),
        (b"ACTIVATION_DENIED: stopped\n".to_vec(), codes::ACTIVATION_DENIED),
        (Vec::new(), codes::ACTIVATION_DENIED),
        (b"GE".to_vec(), codes::ACTIVATION_DENIED),
        (b"garbage without newline".to_vec(), codes::ACTIVATION_DENIED),
    ];
    for (writes, code) in cases {
        let fake = Fake::with(vec![Step::Pipe { writes: writes.clone(), keep: false, image: Some(shop.clone()) }]);
        let c = connector(&s.0, fake.clone());
        let err = c.dial(&addr("appmcp://shop"), Duration::from_secs(5)).await.expect_err("应被拒绝");
        assert_eq!(err.code, code, "{:?} → {err}", String::from_utf8_lossy(&writes));
        assert_eq!(fake.activations.load(Ordering::SeqCst), 0, "管道已存在时不激活");
    }
}

#[tokio::test]
async fn dial_checks_server_executable() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    for image in [Some(r"C:\evil\shop.exe".to_owned()), None] {
        let fake = Fake::with(vec![Step::Pipe { writes: HELLO.to_vec(), keep: true, image }]);
        let err = connector(&s.0, fake).dial(&addr("appmcp://shop"), Duration::from_secs(5)).await.expect_err("应拒绝");
        assert_eq!(err.code, codes::PEER_IDENTITY_MISMATCH, "{err}");
    }
    // 登记中没有 executable 时不核对映像。
    write_reg(&s.0, "loose", kinds::URI, "loose-app", None);
    let fake = Fake::with(vec![Step::Pipe { writes: HELLO.to_vec(), keep: true, image: None }]);
    connector(&s.0, fake).dial(&addr("appmcp://loose"), Duration::from_secs(5)).await.expect("不核对映像");
}

#[tokio::test]
async fn dial_errors_without_activation() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    let fake = Fake::with(vec![]);
    let c = connector(&s.0, fake.clone());
    // 没有登记文件。
    let err = c.dial(&addr("appmcp://shop"), Duration::from_secs(1)).await.expect_err("未登记");
    assert_eq!(err.code, codes::NAME_NOT_FOUND);
    // 登记的程序已不存在：按未安装（spec/naming.md 5.4）。
    write_reg(&s.0, "gone", kinds::EXEC, "", Some(&s.0.join("missing.exe").display().to_string()));
    let err = c.dial(&addr("appmcp://gone"), Duration::from_secs(1)).await.expect_err("程序不存在");
    assert_eq!(err.code, codes::NAME_NOT_FOUND);
    // 不可激活且未运行。
    write_reg(&s.0, "manual", kinds::NONE, "", Some(&shop));
    let err = c.dial(&addr("appmcp://manual"), Duration::from_secs(1)).await.expect_err("不可激活");
    assert_eq!(err.code, codes::ACTIVATION_DENIED);
    // 实例名字不激活。
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let err = c.dial(&addr("appmcp://shop/w2"), Duration::from_secs(1)).await.expect_err("实例未运行");
    assert_eq!(err.code, codes::NAME_NOT_FOUND);
    assert_eq!(fake.activations.load(Ordering::SeqCst), 0);
    // 激活本身失败（程序不存在）原样返回。
    *lock(&fake.activate_error) = Some(ConnectorError::new(codes::NAME_NOT_FOUND, "x"));
    let err = c.dial(&addr("appmcp://shop"), Duration::from_secs(1)).await.expect_err("激活失败");
    assert_eq!(err.code, codes::NAME_NOT_FOUND);
}

#[tokio::test]
async fn watch_turns_directory_changes_into_events() {
    let s = scratch();
    let shop = exe(&s.0, "shop.exe");
    write_reg(&s.0, "shop", kinds::EXEC, "", Some(&shop));
    let fake = Fake::with(vec![]);
    let c = connector(&s.0, fake.clone());
    let mut events = c.watch().await.expect("订阅");
    assert_eq!(c.discover().await.expect("发现").len(), 1);
    let tx = lock(&fake.watch_tx).clone().expect("监视已建立");
    let apps = s.0.join("apps");

    // 新登记 → Installed；临时文件被忽略。
    write_reg(&s.0, "linky", kinds::URI, "linky-app", None);
    tx.send(DirChange::Files(vec!["linky.json".into(), "linky.tmp-9".into()])).expect("发送");
    match events.next().await {
        Some(NameEvent::Installed(n)) => assert_eq!((n.address.app_id.as_str(), n.activatable), ("linky", true)),
        other => panic!("应为 Installed：{other:?}"),
    }
    // 删除 → Removed。
    std::fs::remove_file(apps.join("linky.json")).expect("删除");
    tx.send(DirChange::Files(vec!["linky.json".into()])).expect("发送");
    assert_eq!(events.next().await, Some(NameEvent::Removed(addr("appmcp://linky"))));
    // 从未见过且无效的文件：不产生事件；溢出后重新枚举：shop 被删 → Removed，新的 packed → Installed。
    std::fs::write(apps.join("junk.json"), "{").expect("写");
    std::fs::remove_file(apps.join("shop.json")).expect("删除");
    write_reg(&s.0, "packed", kinds::AUMID, "Co.P_1!App", None);
    tx.send(DirChange::Files(vec!["junk.json".into()])).expect("发送");
    tx.send(DirChange::Rescan).expect("发送");
    let mut got = vec![events.next().await, events.next().await];
    got.sort_by_key(|e| format!("{e:?}"));
    assert!(matches!(&got[0], Some(NameEvent::Installed(n)) if n.address.app_id == "packed"), "{got:?}");
    assert_eq!(got[1], Some(NameEvent::Removed(addr("appmcp://shop"))));
    // 事件流被丢弃即停止监视。
    assert!(!fake.watch_stopped.load(Ordering::SeqCst));
    drop(events);
    assert!(fake.watch_stopped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn manifest_comes_from_registration() {
    let s = scratch();
    let path = s.0.join("app-mcp.json");
    let text = |id: &str| {
        format!(r#"{{"manifestVersion":1,"appId":"{id}","name":"店","tools":[{{"name":"a","description":"d","inputSchema":{{"type":"object"}}}}]}}"#)
    };
    std::fs::write(&path, text("shop")).expect("写清单");
    let mut reg = registration::parse(
        &format!(
            r#"{{"registrationVersion":1,"appId":"shop","source":"manual","manifest":{},"activation":{{"kind":"uri","target":"shop-app"}}}}"#,
            serde_json::to_string(&path.display().to_string()).expect("json")
        ),
        "shop",
    )
    .expect("登记");
    std::fs::write(s.0.join("apps/shop.json"), serde_json::to_string(&reg).expect("序列化")).expect("写");
    let c = connector(&s.0, Fake::with(vec![]));
    assert_eq!(c.manifest("shop").map(|m| m.tools.len()), Some(1));
    std::fs::write(&path, text("other")).expect("改清单");
    assert!(c.manifest("shop").is_none(), "清单 appId 不一致时忽略");
    reg.manifest = None;
    std::fs::write(s.0.join("apps/shop.json"), serde_json::to_string(&reg).expect("序列化")).expect("写");
    assert!(c.manifest("shop").is_none());
    assert!(c.manifest("nobody").is_none());
}

fn notify_record(next: u32, name: &str) -> Vec<u8> {
    let wide: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut rec = Vec::new();
    rec.extend_from_slice(&next.to_le_bytes());
    rec.extend_from_slice(&1u32.to_le_bytes());
    rec.extend_from_slice(&u32::try_from(wide.len()).expect("长度").to_le_bytes());
    rec.extend_from_slice(&wide);
    while rec.len() % 4 != 0 {
        rec.push(0);
    }
    rec
}

#[test]
fn parse_notify_reads_records_and_tolerates_truncation() {
    let second = notify_record(0, "商城.json");
    let first_len = notify_record(0, "shop.json").len();
    let mut bytes = notify_record(u32::try_from(first_len).expect("长度"), "shop.json");
    bytes.extend_from_slice(&second);
    assert_eq!(parse_notify(&bytes), vec!["shop.json".to_owned(), "商城.json".to_owned()]);
    // 截断：只返回完整的记录，不越界。
    assert_eq!(parse_notify(&bytes[..first_len + 13]), vec!["shop.json".to_owned()]);
    assert!(parse_notify(&bytes[..5]).is_empty());
    assert!(parse_notify(&[]).is_empty());
    // NextEntryOffset 指向缓冲外：停止。
    let bad = notify_record(4096, "a.json");
    assert_eq!(parse_notify(&bad), vec!["a.json".to_owned()]);
}
