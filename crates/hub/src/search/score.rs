//! 分词与打分（spec/hub-api.md 3.18「分词」「关键词得分」「排序加成」）。纯函数，不读状态。

use std::cmp::Ordering;
use std::collections::HashSet;

/// 工具参与匹配的文本（原样；匹配前统一转小写）。
#[derive(Clone, Debug, Default)]
pub(crate) struct ToolText<'a> {
    /// 全名 `<appId>.<name>`。
    pub full_name: &'a str,
    pub title: Option<&'a str>,
    pub description: &'a str,
    /// App 名称、页面标题与描述。
    pub context: Vec<&'a str>,
}

/// 匹配字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    FullName,
    Title,
    Description,
    Context,
}

/// 各字段的权重：同一词按出现的字段中最高的一项计，不累加。
pub(crate) const FIELD_WEIGHTS: [(Field, f64); 4] =
    [(Field::FullName, 3.0), (Field::Title, 2.0), (Field::Description, 1.0), (Field::Context, 1.0)];

/// 统计成功率加成所需的最少调用数。
pub(crate) const MIN_CALLS_FOR_RATE: u64 = 3;
/// 成功率不低于此值时加分。
pub(crate) const GOOD_RATE: f64 = 0.8;
/// 成功率低于此值时减分。
pub(crate) const POOR_RATE: f64 = 0.5;

/// 排序加成的输入。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Signals {
    /// 工具已注册在可见 / 有焦点的实例上（当前界面）。
    pub on_current_surface: bool,
    /// 调用方 24 小时内用过。
    pub used_recently: bool,
    /// 调用方的调用数。
    pub calls: u64,
    /// 调用方的成功率（无调用时为 `None`）。
    pub success_rate: Option<f64>,
    /// 工具已弃用（spec/hub-api.md 3.21：得分 −1，仍可检索到）。
    pub deprecated: bool,
}

/// 一条排序加成规则。
pub(crate) struct BonusRule {
    pub applies: fn(&Signals) -> bool,
    pub delta: f64,
}

fn rate_at_least_calls(s: &Signals, pred: fn(f64) -> bool) -> bool {
    s.calls >= MIN_CALLS_FOR_RATE && s.success_rate.is_some_and(pred)
}

/// 排序加成（只加给关键词得分 > 0 的工具）。
pub(crate) const BONUS_RULES: [BonusRule; 5] = [
    BonusRule { applies: |s| s.on_current_surface, delta: 1.0 },
    BonusRule { applies: |s| s.used_recently, delta: 1.0 },
    BonusRule { applies: |s| rate_at_least_calls(s, |r| r >= GOOD_RATE), delta: 0.5 },
    BonusRule { applies: |s| rate_at_least_calls(s, |r| r < POOR_RATE), delta: -0.5 },
    BonusRule { applies: |s| s.deprecated, delta: -1.0 },
];

/// 字符类别：词字符、CJK 字符、分隔符。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Cjk,
    Separator,
}

/// CJK 统一表意文字（含扩展 A、兼容、扩展 B 及以后）、日文假名、韩文音节。
const CJK_RANGES: [(u32, u32); 7] = [
    (0x3040, 0x30FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xAC00, 0xD7AF),
    (0xF900, 0xFAFF),
    (0x20000, 0x2FA1F),
    (0x30000, 0x3134F),
];

fn is_cjk(c: char) -> bool {
    let c = u32::from(c);
    CJK_RANGES.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c))
}

fn classify(c: char) -> CharClass {
    if is_cjk(c) {
        CharClass::Cjk
    } else if c.is_alphanumeric() {
        CharClass::Word
    } else {
        CharClass::Separator
    }
}

/// 一段同类字符切出的词：词字符段长度 1 丢弃；CJK 段切为相邻二字组，单字作为一个词。
fn run_tokens(class: CharClass, run: &[char]) -> Vec<String> {
    match (class, run.len()) {
        (CharClass::Separator, _) | (_, 0) => Vec::new(),
        (CharClass::Word, 1) => Vec::new(),
        (CharClass::Word, _) => vec![run.iter().collect()],
        (CharClass::Cjk, 1) => vec![run.iter().collect()],
        (CharClass::Cjk, _) => run.windows(2).map(|w| w.iter().collect()).collect(),
    }
}

/// 分词：转小写后按字符类别切段，去重（保持首次出现的顺序）。
///
/// @compat 规范只写了 ASCII 字母数字；其他文字的字母数字（如西里尔、带重音的拉丁字母）同样按词处理，避免这些语言的查询全被丢弃。
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut run: Vec<char> = Vec::new();
    let mut class = CharClass::Separator;
    let mut flush = |class: CharClass, run: &mut Vec<char>, out: &mut Vec<String>| {
        for t in run_tokens(class, run) {
            if seen.insert(t.clone()) {
                out.push(t);
            }
        }
        run.clear();
    };
    for c in lower.chars() {
        let next = classify(c);
        if next != class {
            flush(class, &mut run, &mut out);
            class = next;
        }
        run.push(c);
    }
    flush(class, &mut run, &mut out);
    out
}

/// 已转小写的工具文本（每个工具只转一次）。
struct LowerText {
    full_name: String,
    title: String,
    description: String,
    context: Vec<String>,
}

impl LowerText {
    fn new(t: &ToolText<'_>) -> Self {
        Self {
            full_name: t.full_name.to_lowercase(),
            title: t.title.unwrap_or_default().to_lowercase(),
            description: t.description.to_lowercase(),
            context: t.context.iter().map(|c| c.to_lowercase()).collect(),
        }
    }

    fn contains(&self, field: Field, token: &str) -> bool {
        match field {
            Field::FullName => self.full_name.contains(token),
            Field::Title => self.title.contains(token),
            Field::Description => self.description.contains(token),
            Field::Context => self.context.iter().any(|c| c.contains(token)),
        }
    }
}

/// 关键词得分：每个词取其出现的字段中最高的权重，再求和。
pub(crate) fn keyword_score(tokens: &[String], text: &ToolText<'_>) -> f64 {
    let lower = LowerText::new(text);
    tokens
        .iter()
        .map(|t| {
            FIELD_WEIGHTS
                .iter()
                .filter(|(f, _)| lower.contains(*f, t))
                .map(|(_, w)| *w)
                .fold(0.0, f64::max)
        })
        .sum()
}

/// 总分：关键词得分为 0 时为 `None`（不返回）；否则加上各条适用的加成。
pub(crate) fn total_score(keyword: f64, signals: &Signals) -> Option<f64> {
    (keyword > 0.0).then(|| {
        BONUS_RULES.iter().filter(|r| (r.applies)(signals)).map(|r| r.delta).fold(keyword, |acc, d| acc + d)
    })
}

/// 排序：总分降序，同分按全名升序。
pub(crate) fn rank_order(a: (f64, &str), b: (f64, &str)) -> Ordering {
    b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        tokenize(s)
    }

    #[test]
    fn tokenize_ascii_splits_lowercases_and_drops_single_chars() {
        assert_eq!(toks("Export_Orders to CSV, a b2"), ["export", "orders", "to", "csv", "b2"]);
        assert_eq!(toks("x y z"), Vec::<String>::new(), "长度 1 的词丢弃");
        assert_eq!(toks("  !!  "), Vec::<String>::new());
        assert_eq!(toks("cart.checkout"), ["cart", "checkout"]);
    }

    #[test]
    fn tokenize_cjk_bigrams_and_single_char() {
        assert_eq!(toks("导出订单"), ["导出", "出订", "订单"]);
        assert_eq!(toks("删"), ["删"], "单个 CJK 字符作为一个词");
        assert_eq!(toks("导出 订"), ["导出", "订"]);
        // 混排：类别变化处切开
        assert_eq!(toks("导出CSV文件"), ["导出", "csv", "文件"]);
        assert_eq!(toks("カート"), ["カー", "ート"], "假名按 CJK 处理");
    }

    #[test]
    fn tokenize_dedups_in_first_seen_order() {
        assert_eq!(toks("Order order ORDER list"), ["order", "list"]);
        assert_eq!(toks("订单 订单"), ["订单"]);
        assert_eq!(toks("订单订单"), ["订单", "单订"]);
    }

    #[test]
    fn tokenize_other_scripts_as_words() {
        assert_eq!(toks("Заказ é"), ["заказ"]);
    }

    fn text<'a>(full: &'a str, title: Option<&'a str>, desc: &'a str, ctx: &[&'a str]) -> ToolText<'a> {
        ToolText { full_name: full, title, description: desc, context: ctx.to_vec() }
    }

    #[test]
    fn field_weights_each_field() {
        let t = text("shop.alpha", Some("Beta"), "gamma", &["Delta 商城"]);
        let score = |q: &str| keyword_score(&tokenize(q), &t);
        assert_eq!(score("alpha"), 3.0, "全名");
        assert_eq!(score("beta"), 2.0, "title");
        assert_eq!(score("gamma"), 1.0, "description");
        assert_eq!(score("delta"), 1.0, "App 名称 / 页面");
        assert_eq!(score("商城"), 1.0);
        assert_eq!(score("zeta"), 0.0);
        assert_eq!(score("alpha beta gamma delta zeta"), 7.0, "各词求和");
        // appId 属于全名
        assert_eq!(score("shop"), 3.0);
    }

    #[test]
    fn same_token_takes_highest_field_only() {
        let t = text("shop.order", Some("Order"), "order the order", &["order"]);
        assert_eq!(keyword_score(&tokenize("order"), &t), 3.0, "不累加");
        let t = text("shop.x", Some("订单列表"), "查看订单", &["订单"]);
        assert_eq!(keyword_score(&tokenize("订单"), &t), 2.0);
        let t = text("shop.x", None, "查看订单", &[]);
        assert_eq!(keyword_score(&tokenize("订单"), &t), 1.0, "无 title");
    }

    #[test]
    fn zero_keyword_score_is_not_returned_even_with_bonuses() {
        let all = Signals { on_current_surface: true, used_recently: true, calls: 10, success_rate: Some(1.0), deprecated: false };
        assert_eq!(total_score(0.0, &all), None);
        assert_eq!(total_score(1.0, &Signals::default()), Some(1.0));
    }

    #[test]
    fn each_bonus_rule() {
        let cases: [(&str, Signals, f64); 11] = [
            ("current-surface", Signals { on_current_surface: true, ..Signals::default() }, 1.0),
            ("recently-used", Signals { used_recently: true, ..Signals::default() }, 1.0),
            ("reliable", Signals { calls: 3, success_rate: Some(0.8), ..Signals::default() }, 0.5),
            ("reliable 调用不足", Signals { calls: 2, success_rate: Some(1.0), ..Signals::default() }, 0.0),
            ("unreliable", Signals { calls: 3, success_rate: Some(0.49), ..Signals::default() }, -0.5),
            ("unreliable 调用不足", Signals { calls: 2, success_rate: Some(0.0), ..Signals::default() }, 0.0),
            ("中间成功率", Signals { calls: 5, success_rate: Some(0.5), ..Signals::default() }, 0.0),
            ("中间成功率 0.79", Signals { calls: 5, success_rate: Some(0.79), ..Signals::default() }, 0.0),
            ("deprecated", Signals { deprecated: true, ..Signals::default() }, -1.0),
            (
                "全部叠加",
                Signals { on_current_surface: true, used_recently: true, calls: 4, success_rate: Some(1.0), deprecated: false },
                2.5,
            ),
            (
                "弃用叠加",
                Signals { on_current_surface: true, used_recently: true, calls: 4, success_rate: Some(1.0), deprecated: true },
                1.5,
            ),
        ];
        for (what, s, delta) in cases {
            assert_eq!(total_score(2.0, &s), Some(2.0 + delta), "{what}");
        }
        // 每条规则至少被上面一个用例单独触发
        let deltas: Vec<f64> = BONUS_RULES.iter().map(|r| r.delta).collect();
        assert_eq!(deltas, [1.0, 1.0, 0.5, -0.5, -1.0]);
        let low = Signals { deprecated: true, ..Signals::default() };
        assert_eq!(total_score(0.5, &low), Some(-0.5), "弃用后总分可为负，仍返回（仍可检索到）");
    }

    #[test]
    fn deterministic_rank_order() {
        let mut v = vec![(1.0, "b.x"), (3.0, "z.y"), (1.0, "a.x"), (3.5, "m.m"), (3.0, "c.y")];
        v.sort_by(|a, b| rank_order(*a, *b));
        assert_eq!(v, [(3.5, "m.m"), (3.0, "c.y"), (3.0, "z.y"), (1.0, "a.x"), (1.0, "b.x")]);
    }
}
