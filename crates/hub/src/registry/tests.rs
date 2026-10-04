use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::{
    AppOverview, ClientKind, ErrorKind, ResourceInfo, ResourcesSyncParams, ToolInfo, ToolsSyncParams, Visibility,
    WakeDescriptor, WakeKind,
};
use serde_json::{Value, json};

use crate::overview::OverviewSource;
use crate::pages;
use crate::connection::Connection;

use super::*;

fn tool(name: &str) -> ToolInfo {
    serde_json::from_value(json!({"name": name, "description": format!("{name} 工具"), "inputSchema": {"type": "object"}}))
        .unwrap()
}

fn res(name: &str) -> ResourceInfo {
    ResourceInfo {
        name: name.into(),
        description: "r".into(),
        mime_type: None,
        realtime: false,
        annotations: None,
        cache: None,
    }
}

fn manifest() -> Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "示例商城",
            "launch": {"web": [{"type": "url", "href": "http://localhost:5173/"}]},
            "tools": [
                {"name": "orders.search", "description": "搜索", "inputSchema": {"type": "object"}},
                {"name": "cart.add", "description": "加购（静态）", "inputSchema": {"type": "object"}}
            ],
            "resources": [{"name": "cart.state", "description": "购物车"}]
        })
        .to_string(),
    )
    .unwrap()
}

fn add(
    reg: &mut Registry,
    app: &str,
    id: &str,
    conn_id: u64,
) -> (Arc<Connection>, Option<Arc<Connection>>) {
    let (conn, rx) = Connection::new(conn_id, format!("t-{conn_id}"));
    std::mem::forget(rx); // 保持通道打开
    let old = reg.add_instance(
        app,
        NewInstance {
            instance_id: id.into(),
            app_name: "Shop".into(),
            client_kind: ClientKind::Web,
            app_version: None,
            title: Some(format!("tab {id}")),
            url: None,
            overview: None,
            pid: None,
            navigate: false,
            conn: conn.clone(),
            wake: None,
        },
    );
    (conn, old)
}

#[test]
fn overview_runtime_beats_manifest() {
    let mut reg = Registry::new();
    let mut m = manifest();
    m.overview = Some(AppOverview {
        summary: "静态简介".into(),
        body: None,
        locale: None,
    });
    reg.set_manifest(m);
    let o = reg.overview("shop").unwrap();
    assert_eq!(
        (o.summary.as_str(), o.source),
        ("静态简介", OverviewSource::Manifest)
    );
    assert_eq!(reg.summaries()[0].summary.as_deref(), Some("静态简介"));

    let (conn, rx) = Connection::new(9, "t-9");
    std::mem::forget(rx);
    reg.add_instance(
        "shop",
        NewInstance {
            instance_id: "a".into(),
            app_name: "Shop".into(),
            client_kind: ClientKind::Web,
            app_version: None,
            title: None,
            url: None,
            overview: Some(AppOverview {
                summary: "运行时简介".into(),
                body: Some("正文".into()),
                locale: None,
            }),
            pid: None,
            navigate: false,
            conn: conn.clone(),
            wake: None,
        },
    );
    let o = reg.overview("shop").unwrap();
    assert_eq!(
        (o.summary.as_str(), o.source),
        ("运行时简介", OverviewSource::Runtime)
    );
    assert_eq!(o.app_name, "示例商城");
    assert_eq!(
        reg.apps_json(&HashMap::new())["apps"][0]["summary"],
        "运行时简介"
    );
    reg.remove_instance("shop", conn.id);
    assert_eq!(
        reg.overview("shop").unwrap().source,
        OverviewSource::Manifest
    );
    assert!(reg.overview("nope").is_none());
}

#[test]
fn static_tools_when_disconnected() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let tools = reg.tools();
    assert_eq!(tools.len(), 2);
    assert!(
        tools
            .iter()
            .all(|t| t.availability == Availability::Disconnected)
    );
    let err = reg.route_tool("shop", "orders.search", None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::AppDisconnected);
    assert!(err.message.contains("http://localhost:5173/"));
    assert_eq!(
        reg.route_tool("shop", "nope", None).unwrap_err().kind,
        ErrorKind::ToolNotFound
    );
    assert_eq!(
        reg.route_tool("other", "x", None).unwrap_err().kind,
        ErrorKind::ToolNotFound
    );
    let resources = reg.resources();
    assert_eq!(resources.len(), 1);
    assert!(!resources[0].available);
}

#[test]
fn connected_union_and_not_registered() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    let (b, _) = add(&mut reg, "shop", "b", 2);
    assert!(reg.sync_tools("shop", a.id, vec![tool("orders.search"), tool("x")]));
    assert!(!reg.sync_tools("shop", a.id, vec![tool("orders.search"), tool("x")]));
    assert!(reg.sync_tools("shop", b.id, vec![tool("x"), tool("y")]));
    let tools = reg.tools();
    let names: Vec<_> = tools
        .iter()
        .map(|t| (t.info.name.as_str(), t.availability))
        .collect();
    assert_eq!(
        names,
        vec![
            ("orders.search", Availability::Available),
            ("x", Availability::Available),
            ("y", Availability::Available),
            ("cart.add", Availability::NotRegistered),
        ]
    );
    let err = reg.route_tool("shop", "cart.add", None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ToolNotFound);
    assert!(err.message.contains("打开对应界面"));
    // 只在注册了工具的实例中选择
    assert_eq!(
        reg.route_tool("shop", "y", Some("a")).unwrap().instance_id,
        "b"
    );
    assert_eq!(
        reg.route_tool("shop", "orders.search", Some("b"))
            .unwrap()
            .instance_id,
        "a"
    );
}

#[test]
fn invalid_tools_are_dropped() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    let mut bad = tool("ok");
    bad.input_schema = json!({"type": "array"});
    bad.name = "bad".into();
    reg.sync_tools("app", a.id, vec![tool("ok"), bad, tool("has space")]);
    assert_eq!(reg.tools().len(), 1);
}

#[test]
fn change_tools_incremental() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    reg.sync_tools("app", a.id, vec![tool("x")]);
    assert!(reg.change_tools("app", a.id, vec![tool("y")], vec!["x".into()]));
    assert!(!reg.change_tools("app", a.id, vec![], vec!["zzz".into()]));
    let names: Vec<_> = reg.tools().into_iter().map(|t| t.info.name.clone()).collect();
    assert_eq!(names, vec!["y"]);
}

#[test]
fn routing_focus_recency_and_connect_order() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    let (b, _) = add(&mut reg, "app", "b", 2);
    for c in [&a, &b] {
        reg.sync_tools("app", c.id, vec![tool("t")]);
    }
    // 都没有活跃记录：最早连接
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
    // b 变为可见：最近活跃
    reg.set_visibility("app", b.id, Visibility::Visible, false);
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
    // a 变为可见：a 更近
    reg.set_visibility("app", a.id, Visibility::Visible, false);
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
    // b 聚焦
    reg.set_visibility("app", b.id, Visibility::Visible, true);
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
    // b 失焦，a 完成一次调用后最近活跃
    reg.set_visibility("app", b.id, Visibility::Hidden, false);
    reg.touch("app", "a");
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
    // 选择优先
    assert_eq!(
        reg.route_tool("app", "t", Some("b")).unwrap().instance_id,
        "b"
    );
}

#[test]
fn view_tools_listed_from_primary_instance_and_pages_learned() {
    let view = |name: &str, page: &str| ToolInfo {
        surface: app_mcp_protocol::ToolSurface::View,
        page: Some(page.into()),
        ..tool(name)
    };
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    let (b, _) = add(&mut reg, "app", "b", 2);
    reg.sync_tools("app", a.id, vec![tool("bg"), view("a.view", "pa")]);
    reg.sync_tools("app", b.id, vec![view("b.view", "pb")]);
    reg.set_visibility("app", b.id, Visibility::Visible, true);
    let names = |reg: &Registry| reg.tools().into_iter().map(|t| t.info.name.clone()).collect::<Vec<_>>();
    // b 聚焦：只列 b 的 view 工具，a 的 app 工具照常
    assert_eq!(names(&reg), ["b.view", "bg"]);
    reg.set_visibility("app", a.id, Visibility::Visible, true);
    reg.set_visibility("app", b.id, Visibility::Hidden, false);
    assert_eq!(names(&reg), ["a.view", "bg"]);
    // 页面目录记下两个实例上报的页面；工具注销后仍在目录中
    reg.sync_tools("app", b.id, vec![]);
    let pages: Vec<String> = reg.pages("app").into_iter().map(|p| p.name).collect();
    assert_eq!(pages, ["pa", "pb"]);
    assert!(!reg.tool_registered("app", "b.view") && reg.tool_registered("app", "a.view"));
    assert!(reg.instance_has_tool("app", "a", "a.view") && !reg.instance_has_tool("app", "b", "a.view"));
    // 休眠快照中的 view 工具不列出
    reg.make_dormant("app", a.id, "r".into(), "h".into(), None);
    assert_eq!(names(&reg), ["bg"]);
    // 导航目标：只选声明了导航能力且已就绪的实例
    assert!(reg.navigation_target("app", None).is_none());
    assert!(reg.wake_plan_app("app", None).is_none(), "仍有已连接实例");
    reg.remove_instance("app", b.id);
    let plan = reg.wake_plan_app("app", None).expect("休眠实例");
    assert_eq!((plan.instance_id.as_deref(), plan.tool.is_none()), (Some("a"), true));
}

#[test]
fn navigation_target_prefers_ready_capable_instances() {
    let mut reg = Registry::new();
    let (conn, rx) = Connection::new(9, "t-9");
    std::mem::forget(rx);
    let new = |id: &str, navigate: bool, conn: Arc<Connection>| NewInstance {
        instance_id: id.into(),
        app_name: "Shop".into(),
        client_kind: ClientKind::Native,
        app_version: None,
        title: None,
        url: None,
        overview: None,
        pid: None,
        navigate,
        conn,
        wake: None,
    };
    reg.add_instance("app", new("x", true, conn.clone()));
    assert!(reg.navigation_target("app", None).is_none(), "未就绪");
    reg.set_ready("app", conn.id);
    assert_eq!(reg.navigation_target("app", None).map(|(id, _)| id).as_deref(), Some("x"));
    let (c2, rx) = Connection::new(10, "t-10");
    std::mem::forget(rx);
    reg.add_instance("app", new("y", false, c2.clone()));
    reg.set_ready("app", c2.id);
    assert_eq!(reg.navigation_target("app", Some("y")).map(|(id, _)| id).as_deref(), Some("x"), "y 不支持导航");
    reg.set_visibility("app", conn.id, Visibility::Frozen, false);
    assert!(reg.navigation_target("app", None).is_none(), "冻结的实例不导航");
}

#[test]
fn repeated_visible_does_not_bump() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    let (b, _) = add(&mut reg, "app", "b", 2);
    for c in [&a, &b] {
        reg.sync_tools("app", c.id, vec![tool("t")]);
    }
    reg.set_visibility("app", a.id, Visibility::Visible, false);
    reg.set_visibility("app", b.id, Visibility::Visible, false);
    reg.set_visibility("app", a.id, Visibility::Visible, false);
    assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
}

#[test]
fn frozen_target_errors() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    reg.sync_tools("app", a.id, vec![tool("t")]);
    reg.set_visibility("app", a.id, Visibility::Frozen, false);
    assert_eq!(
        reg.route_tool("app", "t", None).unwrap_err().kind,
        ErrorKind::InstanceFrozen
    );
}

#[test]
fn replace_same_instance_id() {
    let mut reg = Registry::new();
    let (a1, _) = add(&mut reg, "app", "a", 1);
    reg.sync_tools("app", a1.id, vec![tool("t")]);
    let (a2, old) = add(&mut reg, "app", "a", 2);
    assert_eq!(old.map(|c| c.id), Some(1));
    // 旧连接断开不影响新实例
    assert!(reg.remove_instance("app", a1.id).is_none());
    assert!(reg.instance("app", "a").is_some());
    assert!(reg.tools().is_empty());
    assert!(reg.remove_instance("app", a2.id).is_some());
    assert!(!reg.has_app("app"));
}

#[test]
fn remove_keeps_manifest_entry() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    reg.remove_instance("shop", a.id);
    assert!(reg.has_app("shop"));
    assert_eq!(reg.tools()[0].availability, Availability::Disconnected);
}

#[test]
fn resources_and_subscriptions() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    assert!(reg.sync_resources("shop", a.id, vec![res("cart.state")]));
    assert_eq!(reg.resources().len(), 1);
    assert!(reg.resources()[0].available);
    assert_eq!(
        reg.route_resource("shop", "cart.state", None)
            .unwrap()
            .instance_id,
        "a"
    );
    assert_eq!(
        reg.route_resource("shop", "x", None).unwrap_err().kind,
        ErrorKind::ResourceNotFound
    );
    reg.mark_subscribed("shop", a.id, "cart.state", true);
    assert!(reg.resource_holders("shop", "cart.state")[0].1);
    assert!(reg.change_resources("shop", a.id, vec![], vec!["cart.state".into()]));
    assert!(reg.resource_holders("shop", "cart.state").is_empty());
    assert!(reg.instance("shop", "a").unwrap().subscriptions.is_empty());
}

#[test]
fn apps_json_shape() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    let (_b, _) = add(&mut reg, "shop", "b", 2);
    reg.sync_tools("shop", a.id, vec![tool("x")]);
    let mut sel = HashMap::new();
    sel.insert("shop".to_string(), "b".to_string());
    let v = reg.apps_json(&sel);
    let app = &v["apps"][0];
    assert_eq!(app["appId"], "shop");
    assert_eq!(app["name"], "示例商城");
    assert_eq!(app["connected"], true);
    assert_eq!(app["staticToolCount"], 2);
    assert_eq!(app["selectedInstanceId"], "b");
    assert_eq!(app["defaultInstanceId"], "b");
    assert_eq!(app["instances"].as_array().unwrap().len(), 2);
    assert_eq!(app["instances"][0]["tools"], json!(["x"]));
    sel.insert("shop".to_string(), "gone".to_string());
    assert_eq!(
        reg.apps_json(&sel)["apps"][0]["selectedInstanceId"],
        Value::Null
    );
}

#[test]
fn dormant_snapshot_listing_and_plans() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    reg.sync_tools("shop", a.id, vec![tool("x"), tool("orders.search")]);
    reg.sync_resources("shop", a.id, vec![res("cart.state")]);
    let hash = app_mcp_protocol::tools_hash(
        &ToolsSyncParams { tools: vec![tool("x"), tool("orders.search")] },
        &ResourcesSyncParams { resources: vec![res("cart.state")] },
    );
    let wake = WakeDescriptor { kind: WakeKind::Uri, target: Some("shop".into()), background: true };
    assert_eq!(
        reg.make_dormant("shop", a.id, "rt".into(), hash.clone(), Some(wake.clone())).as_deref(),
        Some("a")
    );
    assert!(reg.make_dormant("shop", a.id, "rt".into(), hash.clone(), None).is_none());
    let d = reg.dormant("shop", "a").unwrap();
    assert_eq!(d.snapshot_hash(), hash);
    // 工具仍列出，标记 Dormant；未注册的静态工具仍为 Disconnected
    let listed: Vec<_> = reg.tools().into_iter().map(|t| (t.info.name.clone(), t.availability)).collect();
    assert_eq!(
        listed,
        vec![
            ("orders.search".to_string(), Availability::Dormant),
            ("x".to_string(), Availability::Dormant),
            ("cart.add".to_string(), Availability::Disconnected),
        ]
    );
    assert!(reg.resources().iter().all(|r| r.available));
    let info = &reg.app_infos(&HashMap::new())[0];
    assert!(!info.connected);
    assert_eq!(info.dormant_instances[0].instance_id, "a");
    assert_eq!(reg.apps_json(&HashMap::new())["apps"][0]["dormant"], true);

    let p = reg.wake_plan_tool("shop", "x", None, false).unwrap();
    assert_eq!((p.instance_id.as_deref(), p.descriptor.as_ref()), (Some("a"), Some(&wake)));
    // 只在清单中的工具：冷启动计划（实例为 None）
    let p = reg.wake_plan_tool("shop", "cart.add", None, false).unwrap();
    assert!(p.instance_id.is_none());
    assert!(reg.wake_plan_tool("shop", "nope", None, false).is_none());
    assert!(reg.wake_plan_tool("shop", "x", Some("zzz"), true).is_none());
    assert!(reg.wake_plan_resource("shop", "cart.state", None).is_some());

    // 回连：已连接实例注册了工具时不需要唤醒
    let snap = reg.take_dormant("shop", "a").unwrap();
    let (a2, _) = add(&mut reg, "shop", "a", 2);
    reg.restore_snapshot("shop", a2.id, &snap);
    assert!(reg.wake_plan_tool("shop", "x", None, false).is_none());
    assert_eq!(reg.route_tool("shop", "x", None).unwrap().instance_id, "a");
}

#[test]
fn dormant_expiry_and_clear() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "app", "a", 1);
    reg.make_dormant("app", a.id, "r".into(), String::new(), None);
    assert!(reg.has_app("app"));
    assert!(reg.expire_dormant(SystemTime::UNIX_EPOCH).is_empty());
    let later = SystemTime::now() + std::time::Duration::from_secs(1);
    assert_eq!(reg.expire_dormant(later), vec![("app".to_string(), "a".to_string())]);
    assert!(!reg.has_app("app"));
    let (b, _) = add(&mut reg, "app", "b", 2);
    reg.make_dormant("app", b.id, "r".into(), String::new(), None);
    assert_eq!(reg.clear_dormant("app"), vec!["b".to_string()]);
}

#[test]
fn wake_target_presence_and_known() {
    let mut reg = Registry::new();
    assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Absent);
    let (a, _) = add(&mut reg, "shop", "a", 1);
    // 已连接、未就绪：握手中
    assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Handshaking);
    assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Handshaking);
    assert_eq!(reg.wake_target_presence("shop", Some("b")), WakeTargetPresence::Absent);
    reg.set_ready("shop", a.id);
    assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Ready("a".into()));
    assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Ready("a".into()));
    assert!(reg.instance_known("shop", "a"));
    // 休眠：仍有记录，但不算已连接
    reg.make_dormant("shop", a.id, "rt".into(), String::new(), None);
    assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Absent);
    assert!(reg.instance_known("shop", "a"));
    reg.clear_dormant("shop");
    assert!(!reg.instance_known("shop", "a"));
    assert!(!reg.instance_known("other", "a"));
}

/// 回归（第 4f 项 d）：同一工具定义在实例、休眠快照、页面目录与列表之间共享（`Arc`），列出 / 计数不深拷贝。
#[test]
fn tool_definitions_are_shared_not_copied() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    let mut page_tool = tool("orders.view");
    page_tool.page = Some("orders".into());
    reg.sync_tools("shop", a.id, vec![tool("orders.search"), page_tool]);
    let live = reg.instance("shop", "a").unwrap().tools["orders.view"].clone();
    let catalog = reg.pages("shop");
    let (_, in_catalog) = pages::find_tool(&catalog, "orders.view").unwrap();
    assert!(Arc::ptr_eq(&live, in_catalog), "页面目录与实例共享定义");
    let listed = reg.tools();
    let listed_view = listed.iter().find(|t| t.info.name == "orders.view").unwrap();
    assert!(Arc::ptr_eq(&live, &listed_view.info), "列表不复制定义");
    assert_eq!(reg.tool_count(), listed.len());
    assert_eq!(reg.tools_of(|app| app == "other").len(), 0);
    assert_eq!(reg.tools_of(|app| app == "shop").len(), listed.len());

    reg.make_dormant("shop", a.id, "rt".into(), String::new(), None);
    let snap = reg.dormant("shop", "a").unwrap();
    assert!(Arc::ptr_eq(&live, &snap.tools["orders.view"]), "休眠快照沿用同一份定义");
    let (b, _) = add(&mut reg, "shop", "a", 2);
    let snap = reg.take_dormant("shop", "a").unwrap();
    reg.restore_snapshot("shop", b.id, &snap);
    assert!(Arc::ptr_eq(&live, &reg.instance("shop", "a").unwrap().tools["orders.view"]), "快速恢复不复制定义");
    // 静态清单的工具也只有一份：列表与路由取到的是同一个。
    let static_listed = reg.tools().into_iter().find(|t| t.info.name == "cart.add").unwrap();
    assert!(Arc::ptr_eq(&static_listed.info, reg.manifest("shop").unwrap().tool("cart.add").unwrap()));
}

fn shared(tools: Vec<ToolInfo>) -> Vec<SharedTool> {
    sanitize_tools("shop", tools)
}

fn described(name: &str, description: &str) -> ToolInfo {
    ToolInfo { description: description.into(), ..tool(name) }
}

/// 全量同步的声明比较（结果缓存失效依据）：此前已知 = 本实例此前的声明 → 其他已连接实例 → 休眠快照 → 清单；
/// 此前未知按不同处理。
#[test]
fn sync_declaration_compared_with_known() {
    let mut reg = Registry::new();
    let (a, _) = add(&mut reg, "shop", "a", 1);
    assert!(reg.tools_declaration_differs("shop", a.id, &shared(vec![tool("x")])), "一无所知：不同");
    assert!(!reg.tools_declaration_differs("shop", a.id, &[]), "空对空：相同");
    reg.sync_tools("shop", a.id, vec![tool("x")]);
    assert!(!reg.tools_declaration_differs("shop", a.id, &shared(vec![tool("x")])), "本实例已同步的列表");

    let (b, _) = add(&mut reg, "shop", "b", 2);
    assert!(!reg.tools_declaration_differs("shop", b.id, &shared(vec![tool("x")])), "其他已连接实例");
    assert!(reg.tools_declaration_differs("shop", b.id, &shared(vec![described("x", "新")])), "定义不等");
    assert!(reg.tools_declaration_differs("shop", b.id, &shared(vec![tool("x"), tool("y")])), "多出未知工具");

    // 同一实例换连接：被替换连接的声明作为基准；少一项即不同；同步后基准清除。
    let (a2, old) = add(&mut reg, "shop", "a", 3);
    assert!(old.is_some());
    assert!(!reg.tools_declaration_differs("shop", a2.id, &shared(vec![tool("x")])));
    assert!(reg.tools_declaration_differs("shop", a2.id, &[]), "本实例此前声明过的被移除");
    reg.sync_tools("shop", a2.id, vec![tool("x")]);
    assert!(reg.instance("shop", "a").is_some_and(|i| i.prior_tools.is_none()), "同步后清除基准");

    // 休眠快照：其他实例的快照，或回连时取出的本实例快照（remember_prior）。
    reg.remove_instance("shop", b.id);
    reg.make_dormant("shop", a2.id, "tok".into(), String::new(), None);
    let (c, _) = add(&mut reg, "shop", "c", 4);
    assert!(!reg.tools_declaration_differs("shop", c.id, &shared(vec![tool("x")])), "其他实例的休眠快照");
    let snap = reg.take_dormant("shop", "a").expect("快照");
    assert!(reg.tools_declaration_differs("shop", c.id, &shared(vec![tool("x")])), "快照已取出：未知");
    reg.remember_prior("shop", c.id, snap);
    assert!(!reg.tools_declaration_differs("shop", c.id, &shared(vec![tool("x")])), "回连时取出的快照");
}

#[test]
fn sync_resource_declaration_compared_with_known() {
    let mut reg = Registry::new();
    reg.set_manifest(manifest());
    let (a, _) = add(&mut reg, "shop", "a", 1);
    let declared = ResourceInfo { description: "购物车".into(), ..res("cart.state") };
    assert!(!reg.resources_declaration_differs("shop", a.id, std::slice::from_ref(&declared)), "与清单声明相同");
    let search = shared(vec![described("orders.search", "搜索")]);
    assert!(!reg.tools_declaration_differs("shop", a.id, &search), "工具与清单声明相同");
    assert!(reg.resources_declaration_differs("shop", a.id, &[res("cart.state")]), "描述与清单不同");
    assert!(reg.resources_declaration_differs("shop", a.id, &[declared.clone(), res("other")]), "未知资源");
    reg.sync_resources("shop", a.id, vec![declared.clone()]);
    let (a2, _) = add(&mut reg, "shop", "a", 2);
    assert!(reg.resources_declaration_differs("shop", a2.id, &[]), "被替换连接声明过的被移除");
    assert!(!reg.resources_declaration_differs("shop", a2.id, &[declared]));
}
