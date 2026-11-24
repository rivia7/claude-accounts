//! 交换密文：keys（记下备查的 API key）、ledger（折合金额）。服务器不懂里面是什么，只管存、按设备和账号给回去、到上限就不收。
//! Python 的 `Store.key_sync`、`cost_sync`、`cost_missing`、`cost_drop`、`cost_prune`（Py:565-649）。
//!
//! 上限在 `Limits` 里：`max_keys`、`cost_slots`、`cost_device_bytes`、`cost_total_bytes`、`cost_resp`。
//!
//! 还没移植：这里是 `impl Store` 的一部分，由 `Store::op` 调。
