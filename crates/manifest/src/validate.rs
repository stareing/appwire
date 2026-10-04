//! 清单语义校验（spec/manifest.md 第 3 节）。

use super::*;

/// 一条校验问题。`path` 为 JSON 路径（如 `tools[1].name`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

impl Issue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// 校验结果：有错误时清单不可用；警告不影响使用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validation {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl Validation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl Manifest {
    /// 按 `spec/manifest.md` 第 3 节校验。
    pub fn validate(&self) -> Validation {
        let mut v = Validation::default();

        if self.manifest_version != MANIFEST_VERSION {
            v.errors.push(Issue::new(
                "manifestVersion",
                format!(
                    "不支持的清单版本 {}，当前只支持 {MANIFEST_VERSION}",
                    self.manifest_version
                ),
            ));
        }
        if !is_valid_app_id(&self.app_id) {
            v.errors.push(Issue::new(
                "appId",
                format!(
                    "appId \"{}\" 格式不合法，应满足 [a-z][a-z0-9-]{{0,62}}",
                    self.app_id
                ),
            ));
        } else if is_reserved_app_id(&self.app_id) {
            v.errors.push(Issue::new(
                "appId",
                format!("appId \"{}\" 是保留名", self.app_id),
            ));
        }
        if self.name.trim().is_empty() {
            v.warnings.push(Issue::new("name", "name 为空"));
        }
        if self.description.as_deref() == Some("") {
            v.errors
                .push(Issue::new("description", "description 不能为空字符串"));
        }
        if let Some(ov) = &self.overview {
            if ov.summary.trim().is_empty() {
                v.errors
                    .push(Issue::new("overview.summary", "overview.summary 不能为空"));
            }
            let n = ov.summary.chars().count();
            if n > OVERVIEW_SUMMARY_MAX_CHARS {
                v.warnings.push(Issue::new(
                    "overview.summary",
                    format!(
                        "summary 有 {n} 个字符，超过 {OVERVIEW_SUMMARY_MAX_CHARS}，Host 会截断"
                    ),
                ));
            }
            if let Some(body) = &ov.body {
                let n = body.chars().count();
                if n > OVERVIEW_BODY_MAX_CHARS {
                    v.warnings.push(Issue::new(
                        "overview.body",
                        format!("body 有 {n} 个字符，超过 {OVERVIEW_BODY_MAX_CHARS}，Host 会截断"),
                    ));
                }
            }
        }

        // 工具名在 App 内唯一：顶层工具与各页面工具共用一个名称空间。
        let mut seen = HashSet::new();
        for (i, tool) in self.tools.iter().enumerate() {
            let path = format!("tools[{i}]");
            self.validate_tool(tool, &path, &mut seen, &mut v);
            if let Some(page) = &tool.page
                && self.page(page).is_none()
            {
                v.warnings.push(Issue::new(
                    format!("{path}.page"),
                    format!("页面 \"{page}\" 未在 pages 中声明"),
                ));
            }
        }
        self.validate_pages(&mut seen, &mut v);

        let mut seen = HashSet::new();
        for (i, res) in self.resources.iter().enumerate() {
            let path = format!("resources[{i}]");
            if !is_valid_name(&res.name) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!(
                        "资源名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}",
                        res.name
                    ),
                ));
            } else if !seen.insert(res.name.as_str()) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!("资源名 \"{}\" 重复", res.name),
                ));
            }
            if res.description.is_empty() {
                v.errors.push(Issue::new(
                    format!("{path}.description"),
                    "description 不能为空字符串",
                ));
            }
            if let Some(cache) = &res.cache {
                validate_cache(cache, &format!("{path}.cache"), &mut v);
            }
        }

        self.validate_events(&mut v);
        self.validate_launch(&mut v);
        self.validate_wake(&mut v);
        v
    }

    /// 事件目录（spec/manifest.md 2.4）：名称合法且唯一、description 非空、payloadSchema 为对象。
    fn validate_events(&self, v: &mut Validation) {
        let mut seen = HashSet::new();
        for (i, event) in self.events.iter().enumerate() {
            let path = format!("events[{i}]");
            if !is_valid_name(&event.name) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!("事件名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}", event.name),
                ));
            } else if !seen.insert(event.name.as_str()) {
                v.errors.push(Issue::new(format!("{path}.name"), format!("事件名 \"{}\" 重复", event.name)));
            }
            if app_mcp_protocol::has_app_id_prefix(&event.name, &self.app_id) {
                v.warnings.push(Issue::new(
                    format!("{path}.name"),
                    app_mcp_protocol::app_id_prefix_warning(&event.name, &self.app_id),
                ));
            }
            if event.description.trim().is_empty() {
                v.errors.push(Issue::new(format!("{path}.description"), "description 不能为空"));
            }
            if event.payload_schema.as_ref().is_some_and(|s| !s.is_object()) {
                v.errors.push(Issue::new(format!("{path}.payloadSchema"), "payloadSchema 必须是对象（JSON Schema）"));
            }
        }
    }

    /// 单个工具（顶层或页面内）的规则；`seen` 为已出现的工具名。
    fn validate_tool<'a>(&self, tool: &'a ToolInfo, path: &str, seen: &mut HashSet<&'a str>, v: &mut Validation) {
        if !is_valid_name(&tool.name) {
            v.errors.push(Issue::new(
                format!("{path}.name"),
                format!("工具名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}", tool.name),
            ));
        } else if !seen.insert(tool.name.as_str()) {
            v.errors.push(Issue::new(format!("{path}.name"), format!("工具名 \"{}\" 重复", tool.name)));
        }
        if app_mcp_protocol::has_app_id_prefix(&tool.name, &self.app_id) {
            v.warnings.push(Issue::new(
                format!("{path}.name"),
                app_mcp_protocol::app_id_prefix_warning(&tool.name, &self.app_id),
            ));
        }
        if tool.description.is_empty() {
            v.errors.push(Issue::new(format!("{path}.description"), "description 不能为空字符串"));
        }
        match tool.input_schema.as_object() {
            None => v.errors.push(Issue::new(format!("{path}.inputSchema"), "inputSchema 必须是对象")),
            Some(obj) => {
                if obj.get("type").and_then(Value::as_str) != Some("object") {
                    v.errors.push(Issue::new(
                        format!("{path}.inputSchema.type"),
                        "inputSchema 的 type 必须为 \"object\"",
                    ));
                }
            }
        }
        // outputSchema 可以是任意根类型（非 object 时 Hub 按 MCP 要求包装，spec/protocol.md 3.2），但必须是 JSON Schema 对象。
        if tool.output_schema.as_ref().is_some_and(|s| !s.is_object()) {
            v.errors.push(Issue::new(format!("{path}.outputSchema"), "outputSchema 必须是对象（JSON Schema）"));
        }
        if let Some(alt) = &tool.background_tool {
            self.validate_background_tool(tool, alt, &format!("{path}.backgroundTool"), v);
        }
        validate_implements(tool, &format!("{path}.implements"), v);
        if let Some(cache) = &tool.cache {
            let at = format!("{path}.cache");
            validate_cache(cache, &at, v);
            if tool.effective_annotations().read_only_hint != Some(true) {
                v.warnings.push(Issue::new(at, "cache 只对只读工具（生效注解 readOnlyHint 为 true）生效，Hub 会忽略此声明"));
            }
        }
    }

    /// `backgroundTool`（spec/manifest.md 2.3）：同一 App 中一个 app 工具的名称。
    fn validate_background_tool(&self, tool: &ToolInfo, alt: &str, path: &str, v: &mut Validation) {
        if !is_valid_name(alt) {
            v.errors.push(Issue::new(path, format!("backgroundTool \"{alt}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}")));
            return;
        }
        if alt == tool.name {
            v.errors.push(Issue::new(path, "backgroundTool 不能指向工具自身"));
            return;
        }
        if tool.surface.is_app() {
            v.warnings.push(Issue::new(path, "backgroundTool 只对 surface 为 view 的工具有意义，此处会被忽略"));
        }
        match self.tool(alt).or_else(|| self.page_tool(alt).map(|(_, t)| t)) {
            Some(t) if !t.surface.is_app() => {
                v.errors.push(Issue::new(path, format!("backgroundTool \"{alt}\" 必须是 surface 为 app 的工具")));
            }
            Some(_) => {}
            None => v.warnings.push(Issue::new(
                path,
                format!("backgroundTool \"{alt}\" 未在清单中声明：只有 App 运行时注册了该工具后 Hub 才会改调"),
            )),
        }
    }

    /// 页面目录（spec/manifest.md 2.3）。
    fn validate_pages<'a>(&'a self, seen_tools: &mut HashSet<&'a str>, v: &mut Validation) {
        let mut seen = HashSet::new();
        for (i, page) in self.pages.iter().enumerate() {
            let path = format!("pages[{i}]");
            if !is_valid_name(&page.name) {
                v.errors.push(Issue::new(
                    format!("{path}.name"),
                    format!("页面名 \"{}\" 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}", page.name),
                ));
            } else if !seen.insert(page.name.as_str()) {
                v.errors.push(Issue::new(format!("{path}.name"), format!("页面名 \"{}\" 重复", page.name)));
            }
            if page.description.as_deref() == Some("") {
                v.errors.push(Issue::new(format!("{path}.description"), "description 不能为空字符串"));
            }
            if let Some(params) = &page.params
                && params.get("type").and_then(Value::as_str) != Some("object")
            {
                v.errors.push(Issue::new(
                    format!("{path}.params"),
                    "params 必须是 type 为 \"object\" 的 JSON Schema 对象",
                ));
            }
            for (j, tool) in page.tools.iter().enumerate() {
                let tpath = format!("{path}.tools[{j}]");
                self.validate_tool(tool, &tpath, seen_tools, v);
                if let Some(p) = &tool.page
                    && p != &page.name
                {
                    v.errors.push(Issue::new(
                        format!("{tpath}.page"),
                        format!("工具声明的页面 \"{p}\" 与所在页面 \"{}\" 不一致", page.name),
                    ));
                }
            }
        }
    }

    fn validate_wake(&self, v: &mut Validation) {
        for platform in WAKE_PLATFORMS {
            for (i, entry) in self.wake.platform(platform).iter().enumerate() {
                let path = format!("wake.{platform}[{i}]");
                match entry {
                    WakeEntry::Known(d) => validate_wake_descriptor(d, platform, &path, v),
                    WakeEntry::Other(value) => match value.get("kind").and_then(Value::as_str) {
                        None => v
                            .errors
                            .push(Issue::new(&path, "唤醒描述必须是带字符串 kind 字段的对象")),
                        Some(k) if KNOWN_WAKE_KINDS.contains(&k) => v.errors.push(Issue::new(
                            &path,
                            format!("唤醒描述 \"{k}\" 的字段类型错误（target 应为字符串，background 应为布尔）"),
                        )),
                        Some(k) => v
                            .warnings
                            .push(Issue::new(&path, format!("未知的唤醒方式 \"{k}\"，已忽略"))),
                    },
                }
            }
        }
        for key in self.wake.other.keys() {
            v.warnings.push(Issue::new(
                format!("wake.{key}"),
                format!("未知的平台 \"{key}\"，已忽略"),
            ));
        }
    }

    fn validate_launch(&self, v: &mut Validation) {
        let platforms: [(&str, &Vec<LaunchEntry>); 4] = [
            ("web", &self.launch.web),
            ("windows", &self.launch.windows),
            ("macos", &self.launch.macos),
            ("linux", &self.launch.linux),
        ];
        for (platform, entries) in platforms {
            for (i, entry) in entries.iter().enumerate() {
                let path = format!("launch.{platform}[{i}]");
                match entry {
                    LaunchEntry::Known(known) => {
                        if !known.platforms().contains(&platform) {
                            v.warnings.push(Issue::new(
                                &path,
                                format!(
                                    "唤醒方式 \"{}\" 不适用于平台 {platform}",
                                    known.type_name()
                                ),
                            ));
                        }
                        if let KnownLaunch::Url { href } = known
                            && !(href.starts_with("http://") || href.starts_with("https://"))
                        {
                            v.warnings.push(Issue::new(
                                &path,
                                format!("href \"{href}\" 不是 http(s) 地址"),
                            ));
                        }
                    }
                    LaunchEntry::Other(value) => match value.get("type").and_then(Value::as_str) {
                        None => v
                            .errors
                            .push(Issue::new(&path, "唤醒条目必须是带字符串 type 字段的对象")),
                        Some(t) if KNOWN_LAUNCH_TYPES.contains(&t) => v.errors.push(Issue::new(
                            &path,
                            format!("唤醒方式 \"{t}\" 缺少必需字段或字段类型错误"),
                        )),
                        Some(t) => v
                            .warnings
                            .push(Issue::new(&path, format!("未知的唤醒方式 \"{t}\"，已忽略"))),
                    },
                }
            }
        }
        for key in self.launch.other.keys() {
            v.warnings.push(Issue::new(
                format!("launch.{key}"),
                format!("未知的平台 \"{key}\"，已忽略"),
            ));
        }
    }
}

/// 工具 `implements`（spec/manifest.md 第 3 节、spec/intents.md 第 1 节）：格式 / 重复 / 上限为错误；动词或版本不在词表中、
/// 不满足词表必填参数为警告（允许 App 先于本库词表声明）。
fn validate_implements(tool: &ToolInfo, path: &str, v: &mut Validation) {
    use app_mcp_protocol::intents::{Compatibility, IntentRef, compatibility, implements_errors};
    let errors = implements_errors(&tool.implements);
    for (index, e) in &errors {
        let at = index.map_or_else(|| path.to_owned(), |i| format!("{path}[{i}]"));
        v.errors.push(Issue::new(at, e.to_string()));
    }
    for (i, item) in tool.implements.iter().enumerate() {
        if errors.iter().any(|(index, _)| *index == Some(i)) {
            continue;
        }
        let Ok(intent) = IntentRef::parse(item) else { continue };
        let at = format!("{path}[{i}]");
        match compatibility(intent, &tool.input_schema) {
            Compatibility::Compatible => {}
            Compatibility::Unknown => v.warnings.push(Issue::new(
                at,
                format!("\"{item}\" 不在标准意图词表中（spec/intents.md 第 2 节）：Hub 照常列出，标注 known: false"),
            )),
            Compatibility::Incompatible(reason) => v.warnings.push(Issue::new(
                at,
                format!("工具不满足 \"{item}\" 的必填参数（{reason}）：Hub 不把它列为实现者，工具本身照常可调用"),
            )),
        }
    }
}

/// `cache`（spec/manifest.md 第 3 节）：`ttlMs` 范围违反为错误；`scope` 取值在解析阶段已限定。
fn validate_cache(cache: &app_mcp_protocol::CachePolicy, path: &str, v: &mut Validation) {
    if let Err(reason) = cache.validate() {
        v.errors.push(Issue::new(format!("{path}.ttlMs"), reason));
    }
}

fn validate_wake_descriptor(d: &WakeDescriptor, platform: &str, path: &str, v: &mut Validation) {
    let kind = d.kind.as_str();
    if !d.kind.platforms().contains(&platform) {
        v.warnings.push(Issue::new(
            path,
            format!("唤醒方式 \"{kind}\" 不适用于平台 {platform}"),
        ));
    }
    let target = d.target.as_deref().map(str::trim);
    if d.kind == WakeKind::None {
        if target.is_some() {
            v.warnings
                .push(Issue::new(path, "唤醒方式 \"none\" 不需要 target，已忽略"));
        }
        return;
    }
    let Some(target) = target.filter(|t| !t.is_empty()) else {
        v.errors.push(Issue::new(
            format!("{path}.target"),
            format!("唤醒方式 \"{kind}\" 缺少 target"),
        ));
        return;
    };
    let target_path = format!("{path}.target");
    match d.kind {
        WakeKind::WebUrl => {
            if !(target.starts_with("http://") || target.starts_with("https://")) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("web-url 的 target \"{target}\" 必须是 http(s) 地址"),
                ));
            } else if target.contains('#') {
                v.warnings.push(Issue::new(
                    target_path,
                    "web-url 的 target 不应包含片段（#），Host 会附加 #app-mcp-wake=<token>",
                ));
            }
            if d.background {
                v.warnings
                    .push(Issue::new(path, "网页无法在后台唤醒，background 将被忽略"));
            }
        }
        WakeKind::Uri => {
            if !is_valid_uri_scheme(target) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("uri 的 target 应为 scheme（如 shop-app），\"{target}\" 不合法"),
                ));
            } else if matches!(
                target.to_ascii_lowercase().as_str(),
                "http" | "https" | "file" | "javascript" | "data"
            ) {
                v.errors.push(Issue::new(
                    target_path,
                    format!("uri 的 target 不能是 \"{target}\"，网页请用 web-url"),
                ));
            }
        }
        WakeKind::AndroidIntent => {
            let valid = target
                .split_once('/')
                .is_some_and(|(pkg, class)| !pkg.is_empty() && !class.is_empty());
            if !valid {
                v.errors.push(Issue::new(
                    target_path,
                    format!(
                        "android-intent 的 target 应为 <包名>/<接收器类名>，实际为 \"{target}\""
                    ),
                ));
            }
        }
        WakeKind::Dbus => {
            if !target.contains('.') {
                v.warnings.push(Issue::new(
                    target_path,
                    format!("D-Bus 名称 \"{target}\" 通常应为反向域名形式"),
                ));
            }
        }
        WakeKind::Aumid | WakeKind::AppleEvent | WakeKind::None => {}
    }
}
