//! Team 的名单：管理员登记设备、发邀请、存各台设备的配置、移除设备；设备拿邀请加入、取配置、看名单、退出。
//! Python 的 `Store.free_name` 到 `team_leave`（Py:651-871）。
//!
//! 上限在 `Limits` 里：`max_team_members`、`max_invites`。
//!
//! 还没移植：这里是 `impl Store` 的一部分，由 `Store::op` 调。
