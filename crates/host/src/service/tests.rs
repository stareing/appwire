use super::unit_files::systemd_quote;
use super::*;

fn unix_spec() -> ServiceSpec {
    ServiceSpec {
        exe: PathBuf::from("/opt/app mcp/bin/app-mcp-host"),
        home: PathBuf::from("/home/u/.app-mcp"),
        on_demand: None,
    }
}

#[test]
fn systemd_unit_snapshot() {
    let expected = r#"# 由 app-mcp-host service install 生成；设置见 /home/u/.app-mcp/config.json。
[Unit]
Description=app-mcp Host（本机 App 的 MCP 服务）
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
ExecStart="/opt/app mcp/bin/app-mcp-host" "serve" "--home" "/home/u/.app-mcp"
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"#;
    assert_eq!(systemd_unit(&unix_spec()), expected);
}


fn on_demand(ipc: Option<&str>) -> OnDemandSockets {
    OnDemandSockets { listen: "127.0.0.1:7717".parse().unwrap(), ipc: ipc.map(PathBuf::from) }
}

#[test]
fn systemd_on_demand_units_snapshot() {
    let spec = ServiceSpec { on_demand: Some(on_demand(Some("/run/user/1000/app-mcp/hub.sock"))), ..unix_spec() };
    let expected = r#"# 由 app-mcp-host service install 生成；设置见 /home/u/.app-mcp/config.json。
[Unit]
Description=app-mcp Host（本机 App 的 MCP 服务）
Requires=app-mcp-host.socket
After=app-mcp-host.socket
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
ExecStart="/opt/app mcp/bin/app-mcp-host" "serve" "--home" "/home/u/.app-mcp"
Restart=on-failure
RestartSec=2
"#;
    assert_eq!(systemd_unit(&spec), expected, "按需启动的服务不随登录启动（没有 [Install]）");
    let expected = r#"# 由 app-mcp-host service install --on-demand 生成；设置见 /home/u/.app-mcp/config.json。
# 首个连接时 systemd 启动 app-mcp-host.service，Host 空闲后退出（lifecycle.idleExitMs）。
[Unit]
Description=app-mcp Host 按需启动套接字

[Socket]
ListenStream=127.0.0.1:7717
ListenStream=/run/user/1000/app-mcp/hub.sock
SocketMode=0600
DirectoryMode=0700
FileDescriptorName=app-mcp-host
Service=app-mcp-host.service

[Install]
WantedBy=sockets.target
"#;
    assert_eq!(systemd_socket_unit(&spec, spec.on_demand.as_ref().unwrap()).unwrap(), expected);

    let no_ipc = on_demand(None);
    let text = systemd_socket_unit(&spec, &no_ipc).unwrap();
    assert_eq!(text.matches("ListenStream=").count(), 1);
    assert!(!text.contains("SocketMode"));
    let v6 = OnDemandSockets { listen: "[::1]:7737".parse().unwrap(), ipc: Some(PathBuf::from("/run/a%b/hub.sock")) };
    let text = systemd_socket_unit(&spec, &v6).unwrap();
    assert!(text.contains("ListenStream=[::1]:7737\n"), "{text}");
    assert!(text.contains("ListenStream=/run/a%%b/hub.sock\n"), "% 是 systemd 说明符：{text}");
    let spaced = OnDemandSockets { ipc: Some(PathBuf::from("/run/a b/hub.sock")), ..on_demand(None) };
    assert!(systemd_socket_unit(&spec, &spaced).is_err(), "ListenStream 不能含空白");
}

#[test]
fn launchd_on_demand_plist_snapshot() {
    let spec = ServiceSpec {
        exe: PathBuf::from("/Users/u/.cargo/bin/app-mcp-host"),
        home: PathBuf::from("/Users/u/.app-mcp"),
        on_demand: Some(on_demand(Some("/Users/u/.app-mcp/run/hub.sock"))),
    };
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.app-mcp.host</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/u/.cargo/bin/app-mcp-host</string>
    <string>serve</string>
    <string>--home</string>
    <string>/Users/u/.app-mcp</string>
  </array>
  <key>Sockets</key>
  <dict>
    <key>Http</key>
    <dict>
      <key>SockNodeName</key>
      <string>127.0.0.1</string>
      <key>SockServiceName</key>
      <string>7717</string>
      <key>SockType</key>
      <string>stream</string>
      <key>SockFamily</key>
      <string>IPv4</string>
    </dict>
    <key>Ipc</key>
    <dict>
      <key>SockPathName</key>
      <string>/Users/u/.app-mcp/run/hub.sock</string>
      <key>SockPathMode</key>
      <integer>384</integer>
    </dict>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/dev/null</string>
  <key>StandardErrorPath</key>
  <string>/dev/null</string>
</dict>
</plist>
"#;
    let text = launchd_plist(&spec);
    assert_eq!(text, expected);
    assert!(!text.contains("RunAtLoad") && !text.contains("KeepAlive"), "按需启动不能隐含 RunAtLoad");
    let v6 = ServiceSpec { on_demand: Some(OnDemandSockets { listen: "[::1]:7737".parse().unwrap(), ipc: None }), ..spec };
    let text = launchd_plist(&v6);
    assert!(text.contains("<string>::1</string>") && text.contains("<string>IPv6</string>"), "{text}");
    assert!(!text.contains("<key>Ipc</key>"));
}

#[test]
fn on_demand_sockets_from_settings() {
    let s = OnDemandSockets::from_settings("127.0.0.1:7717", Some("unix:/run/user/1/app-mcp/hub.sock")).unwrap();
    assert_eq!(s, on_demand(Some("/run/user/1/app-mcp/hub.sock")));
    assert_eq!(OnDemandSockets::from_settings("127.0.0.1:7717", None).unwrap().ipc, None);
    assert!(OnDemandSockets::from_settings("localhost:7717", None).is_err(), "服务管理器需要 IP");
    assert!(OnDemandSockets::from_settings("127.0.0.1:0", None).is_err(), "systemd 不绑定端口 0");
    assert!(OnDemandSockets::from_settings("127.0.0.1:7717", Some(r"pipe:\\.\pipe\x")).is_err());
    assert!(OnDemandSockets::from_settings("127.0.0.1:7717", Some("ws://127.0.0.1:1")).is_err());
}

#[test]
fn systemd_quoting() {
    assert_eq!(systemd_quote(r#"a"b\c%d$e"#), r#""a\"b\\c%%d$$e""#);
}

#[test]
fn launchd_plist_snapshot() {
    let spec = ServiceSpec {
        exe: PathBuf::from("/Users/u/.cargo/bin/app-mcp-host"),
        home: PathBuf::from("/Users/u/.app-mcp & co"),
        on_demand: None,
    };
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.app-mcp.host</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/u/.cargo/bin/app-mcp-host</string>
    <string>serve</string>
    <string>--home</string>
    <string>/Users/u/.app-mcp &amp; co</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/dev/null</string>
  <key>StandardErrorPath</key>
  <string>/dev/null</string>
</dict>
</plist>
"#;
    assert_eq!(launchd_plist(&spec), expected);
}

#[test]
fn windows_run_snapshot() {
    let spec = ServiceSpec {
        exe: PathBuf::from(r"C:\Program Files\app-mcp\app-mcp-hostw.exe"),
        home: PathBuf::from(r"C:\Users\Zhang San\.app-mcp"),
        on_demand: None,
    };
    assert_eq!(
        windows_run_command(&spec),
        r#""C:\Program Files\app-mcp\app-mcp-hostw.exe" serve --home "C:\Users\Zhang San\.app-mcp""#
    );
    let spec = ServiceSpec {
        exe: PathBuf::from(r"D:\tools\app-mcp-hostw.exe"),
        home: PathBuf::from(r"D:\cfg\"),
        on_demand: None,
    };
    assert_eq!(
        windows_run_command(&spec),
        r#""D:\tools\app-mcp-hostw.exe" serve --home D:\cfg\"#
    );
}

#[test]
fn windows_quoting_rules() {
    assert_eq!(windows_quote("plain"), "plain");
    assert_eq!(windows_quote(""), r#""""#);
    assert_eq!(windows_quote(r"a b\"), r#""a b\\""#);
    assert_eq!(windows_quote(r#"say "hi""#), r#""say \"hi\"""#);
    assert_eq!(windows_quote(r#"a\"b c"#), r#""a\\\"b c""#);
}

#[test]
fn verbatim_prefix() {
    assert_eq!(
        strip_verbatim(Path::new(r"\\?\D:\a\b.exe")),
        PathBuf::from(r"D:\a\b.exe")
    );
    assert_eq!(
        strip_verbatim(Path::new(r"\\?\UNC\srv\x")),
        PathBuf::from(r"\\?\UNC\srv\x")
    );
    assert_eq!(
        strip_verbatim(Path::new("/usr/bin/x")),
        PathBuf::from("/usr/bin/x")
    );
}

#[test]
fn background_exe() {
    let exe = Path::new("/x/app-mcp-host");
    if cfg!(windows) {
        assert!(background_exe_for(exe).ends_with(WINDOWS_BACKGROUND_EXE));
    } else {
        assert_eq!(background_exe_for(exe), exe);
    }
}
