//! `@InsightIntentEntity` 类的 ArkTS 输出（自定义意图的对象参数与标准意图的位置实体共用）。
//!
//! 依据（OpenHarmony SDK 6.0.0.47 / API 20）：
//! - `@ohos.app.ability.InsightIntentDecorator.d.ts` 的 `IntentEntityDecoratorInfo`（`entityCategory` 必填、`parameters` 可选）；
//! - `@ohos.app.ability.insightIntent.d.ts` 的 `IntentEntity`（`entityId: string`）；
//! - ets-loader `parseUserIntents.js` 的 `analyzeBaseClass`（实体类须 `implements insightIntent.IntentEntity`，否则 10110021）
//!   与 `isEntity`（执行器的 `object` 属性须是带 `@InsightIntentEntity` 装饰器的类，否则 10110009）。

use crate::code::{Code, string_literal};
use crate::ident::Lang;

/// 实体类的一个属性（均为可选属性，由系统入口赋值）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityProp {
    pub name: String,
    /// ArkTS 类型。
    pub ty: String,
    pub doc: Vec<String>,
}

impl EntityProp {
    pub fn new(name: impl Into<String>, ty: impl Into<String>, doc: Vec<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            doc,
        }
    }
}

/// `IntentEntity` 要求的属性名；实体类总是声明它。
pub const ENTITY_ID: &str = "entityId";

/// 输出 `@InsightIntentEntity` 类（`export class <class> implements insightIntent.IntentEntity`）。
///
/// @input 文件须导入 `insightIntent` 与 `InsightIntentEntity`（`@kit.AbilityKit`）。
/// @invariant 不输出实体 `parameters`：构建工具只在给出时才按其校验实体属性。
pub fn entity_class(
    c: &mut Code,
    doc: &[String],
    category: &str,
    class: &str,
    props: &[EntityProp],
) {
    c.block_doc(doc);
    c.open("@InsightIntentEntity({");
    c.line(format!(
        "entityCategory: {},",
        string_literal(Lang::TypeScript, category)
    ));
    c.close("})");
    c.open(format!(
        "export class {class} implements insightIntent.IntentEntity {{"
    ));
    c.line(format!("public {ENTITY_ID}: string = '';"));
    for p in props {
        c.block_doc(&p.doc);
        c.line(format!("public {}?: {};", p.name, p.ty));
    }
    c.close("}");
}
