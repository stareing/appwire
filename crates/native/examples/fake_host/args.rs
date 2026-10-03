//! 命令行参数解析（`--invoke`、`--ipc`、一致性用例参数等）。

use super::*;

impl Op {
    pub(super) fn invoke(name: impl Into<String>, args: Value) -> Self {
        Self::Invoke { name: name.into(), args, opts: InvokeOpts::default() }
    }
}

fn parse_u64(flag: &str, text: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{flag} 不是合法整数：{text}"))
}

pub(super) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options {
        addr: None,
        ipc: None,
        ops: Vec::new(),
        timeout_ms: 10_000,
        lease_ms: None,
        reject_sleep_ms: None,
        tool_info: false,
        trace: false,
        case: None,
        sdk: None,
        report_dir: None,
        skip: None,
    };
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| format!("{flag} 缺少参数值"));
        match flag.as_str() {
            "--addr" => opts.addr = Some(value("--addr")?),
            "--ipc" => {
                let text = value("--ipc")?;
                let endpoint = Endpoint::parse(&text)?;
                if !endpoint.is_ipc() {
                    return Err(format!("--ipc 需要 unix: 或 pipe: 端点：{text}"));
                }
                opts.ipc = Some(endpoint);
            }
            "--invoke" => opts.ops.push(Op::invoke(value("--invoke")?, json!({}))),
            "--args" => {
                let text = value("--args")?;
                let parsed: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("--args 不是合法 JSON：{e}"))?;
                match opts.ops.last_mut() {
                    Some(Op::Invoke { args, .. }) => *args = parsed,
                    _ => return Err("--args 必须紧跟在 --invoke <tool> 之后".to_owned()),
                }
            }
            "--call-id" | "--invoke-timeout-ms" | "--cancel-after-ms" | "--idempotency-key" => {
                let text = value(&flag)?;
                let Some(Op::Invoke { opts: inv, .. }) = opts.ops.last_mut() else {
                    return Err(format!("{flag} 必须跟在 --invoke <tool> 之后"));
                };
                match flag.as_str() {
                    "--call-id" => inv.call_id = Some(text),
                    "--idempotency-key" => inv.idempotency_key = Some(text),
                    "--invoke-timeout-ms" => inv.timeout_ms = Some(parse_u64(&flag, &text)?),
                    _ => inv.cancel_after_ms = Some(parse_u64(&flag, &text)?),
                }
            }
            "--catalog" => opts.ops.push(Op::Catalog {
                settle_ms: parse_u64("--catalog", &value("--catalog")?)?,
            }),
            "--delay" => opts.ops.push(Op::Delay { ms: parse_u64("--delay", &value("--delay")?)? }),
            "--navigate" => opts.ops.push(Op::Navigate { page: value("--navigate")?, params: None }),
            "--nav-params" => {
                let text = value("--nav-params")?;
                let parsed: Value =
                    serde_json::from_str(&text).map_err(|e| format!("--nav-params 不是合法 JSON：{e}"))?;
                match opts.ops.last_mut() {
                    Some(Op::Navigate { params, .. }) => *params = Some(parsed),
                    _ => return Err("--nav-params 必须紧跟在 --navigate <page> 之后".to_owned()),
                }
            }
            "--trace" => opts.trace = true,
            "--case" => opts.case = Some(PathBuf::from(value("--case")?)),
            "--sdk" => opts.sdk = Some(value("--sdk")?),
            "--report-dir" => opts.report_dir = Some(PathBuf::from(value("--report-dir")?)),
            "--skip" => opts.skip = Some(value("--skip")?),
            "--read" => opts.ops.push(Op::Read {
                name: value("--read")?,
            }),
            "--await-sleep" => opts.ops.push(Op::AwaitSleep),
            "--wake" => {
                if opts.ops.last() != Some(&Op::AwaitSleep) {
                    return Err("--wake 必须紧跟在 --await-sleep 之后".to_owned());
                }
                opts.ops.push(Op::Wake);
            }
            "--tool-info" => opts.tool_info = true,
            "--lease-ms" => opts.lease_ms = Some(parse_u64("--lease-ms", &value("--lease-ms")?)?),
            "--reject-sleep" => {
                opts.reject_sleep_ms = Some(parse_u64("--reject-sleep", &value("--reject-sleep")?)?)
            }
            "--timeout-ms" => opts.timeout_ms = parse_u64("--timeout-ms", &value("--timeout-ms")?)?,
            other => return Err(format!("未知参数：{other}")),
        }
    }
    if opts.addr.is_some() && opts.ipc.is_some() {
        return Err("--addr 与 --ipc 不能同时使用".to_owned());
    }
    Ok(opts)
}
