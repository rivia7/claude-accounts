//! router_merge：多设备共用一个路由器时把本机账号合并进路由器现有的并集，所有权靠路由器上的 owners（槽位 → 设备标识）认。
//! 单设备（owners 全是我的 / 读不到）逐字节等于 router_accounts；多设备保留别的设备的槽位、本机端口写回并集槽位。
//! 临时 HOME、cca 的辅助程序（cca-core router-merge：stdin 是从路由器回读来的 accounts.yaml 和 owners），不联网。
//! （原来是 tests/router_merge.py）

use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::sandbox::{self, ran, Sandbox};
use kit::{case, fsx, list, obj, py};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "router_merge",
    src: include_str!("router_merge.rs"),
    default: true,
    tests: &[
        case!(single_device_none),
        case!(single_device_own_remote),
        case!(owners_absent_overwrite),
        case!(single_device_removed),
        case!(two_devices),
        case!(shared_account),
        case!(other_devices_nodes_join_the_list),
    ],
};

/// 另一台设备的标识
const OTHER: &str = "ffff0000ffff0000";
/// 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身（合并只读写 ~/.cca 里的文件，一个都不该调到）
const GUARDED: &[&str] = &[
    "security",
    "osascript",
    "open",
    "pbcopy",
    "launchctl",
    "ssh",
    "scp",
    "curl",
    "git",
    "nc",
    "crontab",
    "defaults",
    "killall",
    "pkill",
    "sudo",
    "lsappinfo",
    "networksetup",
    "scutil",
    "codesign",
    "spctl",
    "hdiutil",
    "ditto",
    "claude",
    "codex",
    "agy",
    "mihomo",
    "ps",
];

/// 一个用例的沙箱：这台设备的账号表、可选节点，最近一次合并时发给路由器的那一行
struct Fx {
    sb: Sandbox,
    accounts: Vec<(String, String)>,
    nodes: Vec<String>,
    sent: String,
}

impl Fx {
    fn new(t: &mut Ctx) -> Fx {
        let mut sb = Sandbox::new(t, "cca-rmerge-");
        std::fs::create_dir(&sb.cca).unwrap();
        sb.guard(t, GUARDED, &[]);
        let (cca, path) = (sb.cca.clone(), format!("{}:/usr/bin:/bin", sb.bin().display()));
        sb.set_env("CCA_HOME", &cca).set_env("CCA_TOOL", "claude").set_env("PATH", path);
        Fx { sb, accounts: Vec::new(), nodes: Vec::new(), sent: String::new() }
    }

    /// 这台设备的账号表（Claude 的官方账号）和 config.json：端口从 7901 起一个账号一个
    fn build(&mut self, accounts: &[(&str, &str)], nodes: &[&str]) {
        self.accounts = accounts.iter().map(|(e, n)| (e.to_string(), n.to_string())).collect();
        self.nodes = nodes.iter().map(|n| n.to_string()).collect();
        let mut ports = json::Obj::new();
        for (i, (email, _)) in accounts.iter().enumerate() {
            ports.insert(*email, 7901 + i);
        }
        let config = obj! { "version" => 2, "nodes" => self.nodes.clone(), "ports" => ports };
        fsx::write(self.sb.cca.join("config.json"), json::dumps(&config, &Style::default()));
        let mut table = json::Obj::new();
        for (email, node) in accounts {
            table.insert(*email, *node);
        }
        fsx::write(self.sb.cca.join("accounts.json"), json::dumps(&Value::Obj(table), &Style::default()));
    }

    fn cfg(&self) -> Value {
        fsx::read_json(self.sb.cca.join("config.json"))
    }

    /// 只有这台设备时路由器上 accounts.yaml 的内容（router_accounts 的输出）。只有官方账号时这一行不能变：
    /// 变了，所有已安装的机器升级后都会重新部署路由器端
    fn router_accounts(&self) -> String {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .enumerate()
            .map(|(i, (email, node))| obj! { "slot" => i + 1, "name" => name(email), "node" => node })
            .collect();
        let doc = obj! { "base" => 7900, "slots" => 8, "nodes" => self.nodes.clone(), "accounts" => accounts };
        json::dumps(&doc, &Style::default().unicode())
    }

    /// 跑一次 router-merge。remote 是回读来的 {"accounts": [...], "owners": {槽位: 设备标识}}，None 是路由器上什么都没有。
    /// stdin 照 bash 的 router_push 拼：accounts.yaml 的内容、一行 ---CCA-OWN---、owners 的内容。
    /// 返回 (accounts 载荷, owners)；载荷那一行的原文留在 self.sent
    fn merge(&mut self, remote: Option<Value>) -> (Value, Value) {
        let remote = remote.unwrap_or_else(|| obj! {});
        let acc = remote.get("accounts").map_or(String::new(), |a| json::dumps(&obj! { "accounts" => a }, &Style::default()));
        let own = remote.get("owners").map_or(String::new(), |o| json::dumps(o, &Style::default()));
        let r =
            ran(self.sb.helper(["router-merge"]).input(format!("{acc}\n---CCA-OWN---\n{own}")).timeout(Duration::from_secs(30)));
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let (sent, rest) = r.out.split_once("\n---CCA-PORTS---\n").unwrap_or_else(|| panic!("没有 ---CCA-PORTS---：{}", r.out));
        self.sent = sent.to_string();
        let owners = rest.split_once("\n---CCA-OWN---\n").unwrap_or_else(|| panic!("没有 ---CCA-OWN---：{rest}")).1;
        (json::parse(&self.sent).unwrap(), json::parse(owners).unwrap())
    }

    /// 这台设备的标识：第一次合并时生成、记进 config.json（这里先对着空的路由器合并一次）
    fn did(&mut self) -> Value {
        self.merge(None);
        self.cfg()["device_id"].clone()
    }
}

/// 账号在路由器端口池里的名字：Claude 的官方账号没有前缀，就是邮箱化成的账号目录名（辅助程序的 acct_id / ident）
fn name(email: &str) -> String {
    sandbox::ident(email)
}

const NODES: &[&str] = &["US-A", "US-B", "US-C", "US-D"];

/// 载荷里槽位是 slot 的那个账号（`{a['slot']: a for a in accounts}[slot]`：同一个槽位有两个时算后一个）
fn by_slot(payload: &Value, slot: i64) -> &Value {
    let accounts = payload["accounts"].list();
    accounts.iter().rev().find(|a| a["slot"] == Value::from(slot)).unwrap_or_else(|| panic!("没有槽位 {slot}：{payload}"))
}

/// 载荷里名字是 name 的那个账号（同名的有两个时算后一个）
fn by_name<'a>(payload: &'a Value, name: &str) -> &'a Value {
    let accounts = payload["accounts"].list();
    accounts.iter().rev().find(|a| a["name"] == Value::from(name)).unwrap_or_else(|| panic!("没有 {name}：{payload}"))
}

fn names(payload: &Value) -> Vec<Value> {
    payload["accounts"].list().iter().map(|a| a["name"].clone()).collect()
}

/// 单设备：回读为空 → accounts 逐字节等于 router_accounts，owners 全标成我这台，并生成 device_id
fn single_device_none(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A"), ("bob@example.com", "US-B")], NODES);
    let want = fx.router_accounts();
    let (_payload, owners) = fx.merge(None);
    assert_eq!(fx.sent, want);
    let c = fx.cfg();
    assert_eq!(owners, obj! { "1" => &c["device_id"], "2" => &c["device_id"] });
    assert!(py::truthy(&c["device_id"]), "{c}");
}

/// 回读到的账号都是我这台的（owners 全是我）：仍然逐字节一样
fn single_device_own_remote(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A"), ("bob@example.com", "US-B")], NODES);
    let me = fx.did();
    let ra = json::parse(&fx.router_accounts()).unwrap();
    let mut owners = json::Obj::new();
    for a in ra["accounts"].list() {
        owners.insert(a["slot"].to_string(), &me);
    }
    let remote = obj! { "accounts" => &ra["accounts"], "owners" => owners };
    let want = fx.router_accounts();
    fx.merge(Some(remote));
    assert_eq!(fx.sent, want);
}

/// owners 读不到（旧版 cca / 全新路由器）：当作全是我的 → 直接覆盖成我当前的账号表
fn owners_absent_overwrite(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A")], NODES);
    let bob = name("bob@example.com");
    // 没有 owners
    let remote = obj! { "accounts" => list![
        obj! { "slot" => 1, "name" => name("alice@example.com"), "node" => "US-A" },
        obj! { "slot" => 2, "name" => bob, "node" => "US-B" },
    ] };
    let (payload, owners) = fx.merge(Some(remote));
    // bob 没有 owners 时当作我的、不在表里 → 覆盖掉
    assert_eq!(names(&payload), [Value::from(name("alice@example.com"))]);
    assert_eq!(owners, obj! { "1" => &fx.cfg()["device_id"] });
}

/// 单设备删了账号：回读里被删的账号 owner 是我 → 认出是我删的，去掉（走单设备覆盖）
fn single_device_removed(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A")], NODES);
    let me = fx.did();
    let (alice, bob) = (name("alice@example.com"), name("bob@example.com"));
    let remote = obj! {
        "accounts" => list![
            obj! { "slot" => 1, "name" => &alice, "node" => "US-A" },
            obj! { "slot" => 2, "name" => &bob, "node" => "US-B" },
        ],
        "owners" => obj! { "1" => &me, "2" => &me },
    };
    let (payload, owners) = fx.merge(Some(remote));
    assert_eq!(names(&payload), [Value::from(&alice)]);
    assert_eq!(owners, obj! { "1" => &me });
}

/// 多设备：别的设备的账号（owner 不是我）在槽位 1，我的接在后面；保留它的槽位、端口写回并集槽位、nodes 取并集
fn two_devices(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A"), ("bob@example.com", "US-B")], NODES);
    let me = fx.did();
    let carol = "carol-on-device-2";
    let remote = obj! {
        "accounts" => list![obj! { "slot" => 1, "name" => carol, "node" => "US-C" }],
        "owners" => obj! { "1" => OTHER },
    };
    let (payload, owners) = fx.merge(Some(remote));
    assert_eq!(by_slot(&payload, 1)["name"], Value::from(carol));
    assert_eq!(by_slot(&payload, 2)["name"], Value::from(name("alice@example.com")));
    assert_eq!(by_slot(&payload, 3)["name"], Value::from(name("bob@example.com")));
    assert_eq!(payload["nodes"], list!["US-A", "US-B", "US-C", "US-D"]);
    assert_eq!(owners, obj! { "1" => OTHER, "2" => &me, "3" => &me });
    assert_eq!(fx.cfg()["ports"], obj! { "alice@example.com" => 7902, "bob@example.com" => 7903 });
}

/// 共用账号（我和另一台都有同一个邮箱）：天然共用一个槽位，不重复；别的设备的保留
fn shared_account(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A"), ("bob@example.com", "US-B")], NODES);
    let me = fx.did();
    let (alice, dave) = (name("alice@example.com"), "dave-on-device-2");
    let remote = obj! {
        "accounts" => list![
            obj! { "slot" => 5, "name" => dave, "node" => "US-D" },
            obj! { "slot" => 6, "name" => &alice, "node" => "US-A" },
        ],
        "owners" => obj! { "5" => OTHER, "6" => OTHER },
    };
    let (payload, owners) = fx.merge(Some(remote));
    // alice 只出现一次
    assert_eq!(names(&payload).iter().filter(|n| **n == Value::from(&alice)).count(), 1);
    // 沿用对方给的槽位（共用、同一个出口）
    assert_eq!(by_name(&payload, &alice)["slot"], Value::from(6));
    assert_eq!(by_name(&payload, dave)["slot"], Value::from(5));
    assert_eq!(fx.cfg()["ports"]["alice@example.com"], Value::from(7906));
    // bob 独有：取最低空槽
    assert_eq!(by_name(&payload, &name("bob@example.com"))["slot"], Value::from(1));
    // 共用槽位的 owner 保持对方的
    assert_eq!(owners["6"], Value::from(OTHER));
    assert_eq!(owners["1"], me);
}

/// 别的设备用着、这台设备没选的节点也进 nodes（接在后面，路由器上它的槽位才选得中）；REJECT 和这台已经有的不重复加。
/// 这一个是改写时加的：上面 two_devices 里别的设备的节点本来就在这台的可选节点里，看不出并没并
fn other_devices_nodes_join_the_list(t: &mut Ctx) {
    let mut fx = Fx::new(t);
    fx.build(&[("alice@example.com", "US-A")], &["US-A", "US-B"]);
    let me = fx.did();
    let remote = obj! {
        "accounts" => list![
            obj! { "slot" => 1, "name" => "carol-on-device-2", "node" => "HK-A" },
            obj! { "slot" => 2, "name" => "dave-on-device-2", "node" => "REJECT" },
            obj! { "slot" => 3, "name" => "erin-on-device-2", "node" => "US-B" },
        ],
        "owners" => obj! { "1" => OTHER, "2" => OTHER, "3" => OTHER },
    };
    let (payload, owners) = fx.merge(Some(remote));
    assert_eq!(payload["nodes"], list!["US-A", "US-B", "HK-A"]);
    let slots: Vec<Value> = payload["accounts"].list().iter().map(|a| list![&a["slot"], &a["node"]]).collect();
    assert_eq!(slots, [list![1, "HK-A"], list![2, "REJECT"], list![3, "US-B"], list![4, "US-A"]]);
    assert_eq!(owners, obj! { "1" => OTHER, "2" => OTHER, "3" => OTHER, "4" => &me });
    // 这台设备自己的可选节点不动
    assert_eq!(fx.cfg()["nodes"], list!["US-A", "US-B"]);
}
