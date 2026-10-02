//! 各子命令的实现：入口 [`crate::main_entry`] 解析命令行后按子命令分派到这里。

pub(crate) mod hub_config;
pub(crate) mod legacy;
pub(crate) mod serve;
pub(crate) mod service_cmd;
pub(crate) mod setup_cmd;
