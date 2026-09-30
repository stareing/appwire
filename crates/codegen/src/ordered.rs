//! 保留对象键顺序的 JSON 树。
//!
//! workspace 中的 serde_json 未启用 `preserve_order`（`Value` 的对象按键排序），而生成代码中的参数
//! 顺序应与清单中声明的顺序一致（App Intents 参数、构造函数参数等）。这里对清单原文再做一次只记录
//! 键顺序的解析，供 [`crate::schema`] 排列字段。

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// 只保留结构与键顺序的 JSON 节点。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Node {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    #[default]
    Scalar,
}

impl Node {
    /// 对象的成员；不是对象或不存在时为 `None`。
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// 数组的元素。
    pub fn index(&self, i: usize) -> Option<&Node> {
        match self {
            Node::Array(items) => items.get(i),
            _ => None,
        }
    }

    /// 对象的键（按原文顺序）。
    pub fn keys(&self) -> Vec<&str> {
        match self {
            Node::Object(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON 值")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Scalar)
    }
    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(Node::Scalar)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Node>()? {
            items.push(item);
        }
        Ok(Node::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries: Vec<(String, Node)> = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, Node>()? {
            // 重复键：与 serde_json 一致，后者覆盖前者（位置取首次出现）
            if let Some(slot) = entries.iter_mut().find(|(key, _)| *key == k) {
                slot.1 = v;
            } else {
                entries.push((k, v));
            }
        }
        Ok(Node::Object(entries))
    }
}

/// 解析 JSON 文本，只保留结构与键顺序。
pub fn parse(text: &str) -> Result<Node, serde_json::Error> {
    serde_json::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_key_order() {
        let node =
            parse(r#"{"z": 1, "a": {"y": [1, {"q": 0, "b": 1}], "b": null}}"#).expect("JSON");
        assert_eq!(node.keys(), ["z", "a"]);
        let a = node.get("a").expect("a");
        assert_eq!(a.keys(), ["y", "b"]);
        assert_eq!(
            a.get("y").and_then(|y| y.index(1)).map(Node::keys),
            Some(vec!["q", "b"])
        );
        assert!(node.get("missing").is_none());
    }
}
