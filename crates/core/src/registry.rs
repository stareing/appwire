//! 本地注册表：scope、工具、资源，以及"Host 视角"的变更追踪。
//!
//! 变更追踪思路：连接成功（全量同步）时记录一份 Host 已知的快照；之后每次变更只把
//! 名称加入脏集合，取事件时逐个名称比较"当前应暴露的内容"与"Host 已知的内容"，
//! 相同则不发送（因此同名先加后删、改了又改回都会自然抵消）。

use crate::vec_map::{VecMap, VecSet};

use app_mcp_protocol as proto;
use proto::{ResourceInfo, ResourcesChangedParams, ResourcesSyncParams, ToolInfo, ToolsChangedParams, ToolsSyncParams};
use serde_json::Value;

use crate::{CoreError, ResourceDef, ResourceId, ScopeId, ToolDef, ToolId, ToolUpdate};

#[derive(Debug)]
struct ScopeEntry {
    name: String,
    parent: Option<ScopeId>,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    next_id: u64,
    scopes: VecMap<ScopeId, ScopeEntry>,
    tools: VecMap<ToolId, ToolDef>,
    tool_names: VecMap<String, ToolId>,
    resources: VecMap<ResourceId, ResourceDef>,
    resource_names: VecMap<String, ResourceId>,

    /// 是否正在追踪增量变更（仅 `Connected` 期间为 true）。
    tracking: bool,
    host_tools: VecMap<String, ToolInfo>,
    host_resources: VecMap<String, ResourceInfo>,
    dirty_tools: VecSet<String>,
    dirty_resources: VecSet<String>,
}

fn tool_info(def: &ToolDef) -> ToolInfo {
    ToolInfo {
        name: def.name.clone(),
        description: def.description.clone(),
        input_schema: def.input_schema.clone(),
        risk: def.risk,
        activation: def.activation,
        title: def.title.clone(),
        annotations: def.annotations.clone(),
        output_schema: def.output_schema.clone(),
    }
}

fn resource_info(def: &ResourceDef) -> ResourceInfo {
    ResourceInfo {
        name: def.name.clone(),
        description: def.description.clone(),
        mime_type: def.mime_type.clone(),
        realtime: def.realtime,
        annotations: def.annotations.clone(),
    }
}

fn validate_schema(schema: &Value) -> Result<(), CoreError> {
    match schema {
        Value::Object(obj) if obj.get("type").and_then(Value::as_str) == Some("object") => Ok(()),
        _ => Err(CoreError::InvalidSchema),
    }
}

fn validate_name(name: &str) -> Result<(), CoreError> {
    if proto::is_valid_name(name) { Ok(()) } else { Err(CoreError::InvalidName(name.to_owned())) }
}

impl Registry {
    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn check_scope(&self, scope: Option<ScopeId>) -> Result<(), CoreError> {
        match scope {
            Some(s) if !self.scopes.contains_key(&s) => Err(CoreError::UnknownScope(s)),
            _ => Ok(()),
        }
    }

    // ---- scope ----------------------------------------------------------

    pub fn create_scope(&mut self, name: &str, parent: Option<ScopeId>) -> Result<ScopeId, CoreError> {
        self.check_scope(parent)?;
        let id = ScopeId(self.alloc_id());
        self.scopes.insert(id, ScopeEntry { name: name.to_owned(), parent });
        Ok(id)
    }

    pub fn scope_name(&self, scope: ScopeId) -> Option<&str> {
        self.scopes.get(&scope).map(|s| s.name.as_str())
    }

    pub fn dispose_scope(&mut self, scope: ScopeId) -> Result<(), CoreError> {
        if !self.scopes.contains_key(&scope) {
            return Err(CoreError::UnknownScope(scope));
        }
        // 收集 scope 及其全部后代。scope 的父级只能是已存在的 scope，
        // 因此按 id 顺序扫描、父在集合中则加入即可覆盖全部后代。
        let mut doomed = VecSet::default();
        doomed.insert(scope);
        let mut grew = true;
        while grew {
            grew = false;
            for (id, entry) in self.scopes.iter() {
                if !doomed.contains(id) && entry.parent.is_some_and(|p| doomed.contains(&p)) {
                    doomed.insert(*id);
                    grew = true;
                }
            }
        }
        let tools: Vec<ToolId> =
            self.tools.iter().filter(|(_, d)| d.scope.is_some_and(|s| doomed.contains(&s))).map(|(id, _)| *id).collect();
        for id in tools {
            self.remove_tool(id);
        }
        let resources: Vec<ResourceId> = self
            .resources
            .iter()
            .filter(|(_, d)| d.scope.is_some_and(|s| doomed.contains(&s)))
            .map(|(id, _)| *id)
            .collect();
        for id in resources {
            self.remove_resource(id);
        }
        for id in doomed {
            self.scopes.remove(&id);
        }
        Ok(())
    }

    // ---- 工具 -----------------------------------------------------------

    pub fn register_tool(&mut self, def: ToolDef) -> Result<ToolId, CoreError> {
        validate_name(&def.name)?;
        validate_schema(&def.input_schema)?;
        self.check_scope(def.scope)?;
        if self.tool_names.contains_key(&def.name) {
            return Err(CoreError::DuplicateName(def.name));
        }
        let id = ToolId(self.alloc_id());
        self.tool_names.insert(def.name.clone(), id);
        self.mark_tool(&def.name);
        self.tools.insert(id, def);
        Ok(id)
    }

    pub fn update_tool(&mut self, tool: ToolId, update: ToolUpdate) -> Result<(), CoreError> {
        if let Some(schema) = &update.input_schema {
            validate_schema(schema)?;
        }
        let def = self.tools.get_mut(&tool).ok_or(CoreError::UnknownTool(tool))?;
        let ToolUpdate { description, input_schema, risk, activation, title, enabled, annotations, output_schema } = update;
        if let Some(v) = description {
            def.description = v;
        }
        if let Some(v) = input_schema {
            def.input_schema = v;
        }
        if let Some(v) = risk {
            def.risk = v;
        }
        if let Some(v) = activation {
            def.activation = v;
        }
        if let Some(v) = title {
            def.title = v;
        }
        if let Some(v) = enabled {
            def.enabled = v;
        }
        if let Some(v) = annotations {
            def.annotations = v;
        }
        if let Some(v) = output_schema {
            def.output_schema = v;
        }
        let name = def.name.clone();
        self.mark_tool(&name);
        Ok(())
    }

    pub fn unregister_tool(&mut self, tool: ToolId) -> Result<(), CoreError> {
        if self.remove_tool(tool) { Ok(()) } else { Err(CoreError::UnknownTool(tool)) }
    }

    fn remove_tool(&mut self, tool: ToolId) -> bool {
        let Some(def) = self.tools.remove(&tool) else { return false };
        self.tool_names.remove(&def.name);
        self.mark_tool(&def.name);
        true
    }

    pub fn tool(&self, tool: ToolId) -> Option<&ToolDef> {
        self.tools.get(&tool)
    }

    pub fn tool_by_name(&self, name: &str) -> Option<(ToolId, &ToolDef)> {
        let id = *self.tool_names.get(name)?;
        self.tools.get(&id).map(|d| (id, d))
    }

    // ---- 资源 -----------------------------------------------------------

    pub fn register_resource(&mut self, def: ResourceDef) -> Result<ResourceId, CoreError> {
        validate_name(&def.name)?;
        self.check_scope(def.scope)?;
        if self.resource_names.contains_key(&def.name) {
            return Err(CoreError::DuplicateName(def.name));
        }
        let id = ResourceId(self.alloc_id());
        self.resource_names.insert(def.name.clone(), id);
        self.mark_resource(&def.name);
        self.resources.insert(id, def);
        Ok(id)
    }

    pub fn unregister_resource(&mut self, resource: ResourceId) -> Result<(), CoreError> {
        if self.remove_resource(resource) { Ok(()) } else { Err(CoreError::UnknownResource(resource)) }
    }

    fn remove_resource(&mut self, resource: ResourceId) -> bool {
        let Some(def) = self.resources.remove(&resource) else { return false };
        self.resource_names.remove(&def.name);
        self.mark_resource(&def.name);
        true
    }

    pub fn resource(&self, resource: ResourceId) -> Option<&ResourceDef> {
        self.resources.get(&resource)
    }

    pub fn resource_by_name(&self, name: &str) -> Option<(ResourceId, &ResourceDef)> {
        let id = *self.resource_names.get(name)?;
        self.resources.get(&id).map(|d| (id, d))
    }

    // ---- 变更追踪 -------------------------------------------------------

    fn mark_tool(&mut self, name: &str) {
        if self.tracking {
            self.dirty_tools.insert(name.to_owned());
        }
    }

    fn mark_resource(&mut self, name: &str) {
        if self.tracking {
            self.dirty_resources.insert(name.to_owned());
        }
    }

    /// Host 视角下某个名称当前应暴露的工具（禁用视为不存在）。
    fn exposed_tool(&self, name: &str) -> Option<ToolInfo> {
        self.tool_by_name(name).filter(|(_, d)| d.enabled).map(|(_, d)| tool_info(d))
    }

    /// 当前的全量同步内容（按注册顺序，只含已启用的工具）。
    pub fn snapshot(&self) -> (ToolsSyncParams, ResourcesSyncParams) {
        let tools: Vec<ToolInfo> = self.tools.values().filter(|d| d.enabled).map(tool_info).collect();
        let resources: Vec<ResourceInfo> = self.resources.values().map(resource_info).collect();
        (ToolsSyncParams { tools }, ResourcesSyncParams { resources })
    }

    /// 生成全量同步内容，并开始追踪增量变更。
    pub fn start_tracking(&mut self) -> (ToolsSyncParams, ResourcesSyncParams) {
        let (ToolsSyncParams { tools }, ResourcesSyncParams { resources }) = self.snapshot();
        self.host_tools = tools.iter().map(|t| (t.name.clone(), t.clone())).collect();
        self.host_resources = resources.iter().map(|r| (r.name.clone(), r.clone())).collect();
        self.dirty_tools.clear();
        self.dirty_resources.clear();
        self.tracking = true;
        (ToolsSyncParams { tools }, ResourcesSyncParams { resources })
    }

    /// 连接断开：丢弃 Host 快照与未发送的变更（重连后全量同步）。
    pub fn stop_tracking(&mut self) {
        self.tracking = false;
        self.host_tools.clear();
        self.host_resources.clear();
        self.dirty_tools.clear();
        self.dirty_resources.clear();
    }

    /// 取出自上次以来的合并变更；没有实际变化的部分为 `None`。
    pub fn take_changes(&mut self) -> (Option<ToolsChangedParams>, Option<ResourcesChangedParams>) {
        let mut tools = ToolsChangedParams::default();
        for name in std::mem::take(&mut self.dirty_tools) {
            let current = self.exposed_tool(&name);
            if current.as_ref() == self.host_tools.get(&name) {
                continue;
            }
            match current {
                Some(info) => {
                    self.host_tools.insert(name, info.clone());
                    tools.upserted.push(info);
                }
                None => {
                    self.host_tools.remove(&name);
                    tools.removed.push(name);
                }
            }
        }
        let mut resources = ResourcesChangedParams::default();
        for name in std::mem::take(&mut self.dirty_resources) {
            let current = self.resource_by_name(&name).map(|(_, d)| resource_info(d));
            if current.as_ref() == self.host_resources.get(&name) {
                continue;
            }
            match current {
                Some(info) => {
                    self.host_resources.insert(name, info.clone());
                    resources.upserted.push(info);
                }
                None => {
                    self.host_resources.remove(&name);
                    resources.removed.push(name);
                }
            }
        }
        let tools = (!tools.upserted.is_empty() || !tools.removed.is_empty()).then_some(tools);
        let resources = (!resources.upserted.is_empty() || !resources.removed.is_empty()).then_some(resources);
        (tools, resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Risk;
    use serde_json::json;

    fn tool(name: &str, scope: Option<ScopeId>) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({"type": "object"}),
            risk: Risk::Read,
            activation: None,
            title: None,
            enabled: true,
            scope,
            annotations: None,
            output_schema: None,
        }
    }

    #[test]
    fn validation() {
        let mut r = Registry::default();
        assert_eq!(r.register_tool(tool("bad name", None)), Err(CoreError::InvalidName("bad name".into())));
        let mut t = tool("a", None);
        t.input_schema = json!({"type": "array"});
        assert_eq!(r.register_tool(t), Err(CoreError::InvalidSchema));
        let mut t = tool("a", None);
        t.input_schema = json!("object");
        assert_eq!(r.register_tool(t), Err(CoreError::InvalidSchema));
        assert_eq!(r.register_tool(tool("a", Some(ScopeId(99)))), Err(CoreError::UnknownScope(ScopeId(99))));
        r.register_tool(tool("a", None)).unwrap();
        assert_eq!(r.register_tool(tool("a", None)), Err(CoreError::DuplicateName("a".into())));
        // 工具与资源可以同名
        r.register_resource(ResourceDef { name: "a".into(), description: "r".into(), mime_type: None, scope: None, realtime: false, annotations: None })
            .unwrap();
    }

    #[test]
    fn scope_dispose_is_recursive() {
        let mut r = Registry::default();
        let root = r.create_scope("root", None).unwrap();
        let child = r.create_scope("child", Some(root)).unwrap();
        let grandchild = r.create_scope("gc", Some(child)).unwrap();
        let other = r.create_scope("other", None).unwrap();
        r.register_tool(tool("t1", Some(root))).unwrap();
        r.register_tool(tool("t2", Some(grandchild))).unwrap();
        r.register_tool(tool("t3", Some(other))).unwrap();
        r.dispose_scope(root).unwrap();
        assert!(r.tool_by_name("t1").is_none());
        assert!(r.tool_by_name("t2").is_none());
        assert!(r.tool_by_name("t3").is_some());
        assert!(r.scope_name(child).is_none());
        assert_eq!(r.scope_name(other), Some("other"));
        assert_eq!(r.dispose_scope(root), Err(CoreError::UnknownScope(root)));
    }
}
