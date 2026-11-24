//! 上报和查看：report（设备报它开着的会话）、prune（过期的记录）、view（别的设备上这些账号在不在用）。
//! Python 的 `Store.report`、`prune`、`row`、`view`（Py:535-563、873-899），和几个小函数 `sessions`、`last_used`、`ago`（Py:387-399）。
//!
//! 还没移植：这里是 `impl Store` 的一部分，由 `Store::op` 调。
