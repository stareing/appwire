# 标准意图（第 16 项 N4）

本文是标准意图词表与声明方式的唯一定义。设计与调研依据见 `docs/plans/16-agent-os.md` N4（U4 结论）。

标准意图是**通用动词**：不同 App 的"发消息""建日程"等工具声明自己实现了同一个动词，Agent 就能"按动作找 App"，而不必
先知道有哪些 App。本库只**传递声明并列出实现者**：Hub 不代 Agent 选 App、不改写参数、不校验工具是否真的实现了动词的语义
（微内核：选哪个 App 是策略，归 Agent；机主可设默认作为提示）。

## 1. 声明

工具（清单 `tools[]` / `pages[].tools[]`，或运行时注册的 `ToolInfo`）可选字段：

```ts
implements?: string[]   // 如 ["message.send@1"]；每项为 "<动词>@<主版本>"，最多 4 项，不重复
```

- 动词名 `<域>.<动作>`，取自第 2 节词表；未知动词或版本：清单校验给出**警告**（允许 App 先于本库词表声明），Hub 照常列出
  但标注 `known: false`。
- **兼容性检查**（Hub 在列出实现者时做，SDK / 清单校验同样给出警告）：词表中该动词版本的**必填参数**必须出现在工具
  `inputSchema.properties` 中，类型（若声明）一致。不满足时该工具不作为实现者列出（`apps.intents` 的 `incompatible` 中列出原因），
  工具本身照常可调用。
- 语义由 App 负责：参数名按词表，额外参数允许；结果形态不限定。

`implements` 进 `toolsHash`（只在非空时序列化，未声明的工具 hash 不变）。

## 2. 词表（版本 1）

参数 schema 是各平台的交集（字段名尽量借用 schema.org / 平台意图的命名）；"必填"之外都可选。时间为 RFC 3339 字符串。

| 动词 | 必填参数 | 可选参数 |
|---|---|---|
| `message.send@1` | `to: string[]`（≥ 1，收件人：联系人名、号码、地址或 App 内 ID）、`text: string` | `subject: string`、`attachments: string[]`（URI 或 Hub 句柄） |
| `calendar.create@1` | `title: string`、`start: string`（date-time） | `end: string`、`allDay: boolean`、`location: string`、`attendees: string[]`、`notes: string` |
| `media.play@1` | `query: string`（要播放什么；有 `uri` 时可为空字符串） | `uri: string`、`kind: "song" \| "album" \| "artist" \| "playlist" \| "podcast" \| "video"` |
| `file.share@1` | `files: string[]`（≥ 1，URI 或 Hub 句柄） | `mimeType: string`、`text: string`、`to: string[]` |
| `link.open@1` | `url: string`（uri） | — |
| `navigation.start@1` | `destination: object`（`name` / `address` / `lat`+`lng` 至少一组） | `mode: "drive" \| "walk" \| "transit" \| "bike"` |

词表的机器可读形式在 `crates/protocol/src/intents.rs`（每个动词版本的必填参数与类型），本表与之一致；新增动词或版本同时改两处。
不兼容的变更只能发新主版本（`@2`），旧版本保留。

## 3. 平台映射（codegen 二期使用，本期只作记录）

| 动词 | Apple App Intents | Android | 鸿蒙 |
|---|---|---|---|
| `message.send` | `.messages.sendMessage`（iOS 27，destination 需实体解析） | `ACTION_SENDTO` / `ACTION_SEND` | 无标准意图，自定义意图 |
| `calendar.create` | `.calendar.createEvent`（iOS 27） | `ACTION_INSERT Events.CONTENT_URI` | 无，自定义意图 |
| `media.play` | `.audio.playAudio`（需 AudioItem 实体） | `MEDIA_PLAY_FROM_SEARCH` | `PlayMusicList` / `PlayAudio` / `PlayVideo`（需 entityId） |
| `file.share` | 无，自定义意图 | `ACTION_SEND` / `ACTION_SEND_MULTIPLE` | `ohos.want.action.sendData`（待核实） |
| `link.open` | `.browser.openURLInTab`（iOS 18） | `ACTION_VIEW https:` | 自定义意图 |
| `navigation.start` | `.maps.startNavigation`（iOS 27） | `ACTION_VIEW geo:` | `StartNavigate`（GCJ02） |

## 4. Hub：`apps.intents` 与机主默认

- **内置工具** `apps.intents {intent?}`（只读、幂等、总是列出、任务级）：返回
  `{intents: [{intent, known, description?, implementations: [{tool, appId, availability, default?}], incompatible?: [{tool, reason}]}], message}`。
  `intent` 省略 = 列出所有有实现者的动词与词表中的全部动词；给出时可写 `message.send`（任意版本）或 `message.send@1`。
  实现者来源与 `apps.search`（spec/hub-api.md 3.18）相同（可见的 App 工具、休眠快照、清单、页面目录；策略 `hide` 过滤），**不唤醒 App**。
  排序：机主默认在前（`default: true`），其余按工具全名。列出的 App 记入调用方的暴露集合（同 `apps.tools`）。
- **机主默认表**：`HubConfig.intent_defaults: BTreeMap<动词, 工具全名>`（Hub API `Hub::set_intent_defaults` 运行时替换）；
  Host 读 `<home>/intents.json`（`{"defaults": {"message.send": "mail.compose.send"}}`，读写方式同 `policy.json`，`reload` 重新加载，
  不合法时启动报错 / reload 保留旧值）。默认只是提示：Hub 不按它路由，Agent 仍按工具全名调用。
- 调用：Agent 用 `apps.intents` 选定工具后照常按全名调用；Hub 不提供"按动词调用"的入口。
