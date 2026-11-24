//! 本机 UDP 走节点（cca local udp）在辅助程序里的那一半：TUN 配置、状态、选节点、要装到 root 那边的文件。
//! 临时 HOME、cca 的辅助程序（cca-core udp-env / udp-node / udp-build / local-set，界面看的那一份经 gui-state，
//! WebRTC 警告后面的补充经 leak-verdict），不碰 /Library、不联网。
//! （原来是 tests/local_udp.py）
//!
//! 原来直接调内部函数的地方这里都经子命令测；辅助程序自己的单元测试（cargo test，cli/src/cmd/corenet/udp.rs）里另有直接调的：
//! 节点名和转发 UDP 的判断（names_and_capability）、从命令行认 root 那边的内核在不在跑（running_is_read_from_the_command_line）、
//! 没设节点时的状态和界面看的那一份（off_without_a_node、view_without_a_node）、TUN 接管的网段怎么切（routes_tile_the_rest_finer_than_full_tunnels）、
//! 给别的 VPN 让开的记录什么时候算数（other_vpn_counts_only_while_the_launch_script_runs）。

use kit::harness::{Ctx, Module, Need};
use kit::json::{self, Style, Value};
use kit::sandbox::{self, ran, shell_vars, Sandbox};
use kit::{case, crypto, fsx, list, obj, plist};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "local_udp",
    src: include_str!("local_udp.rs"),
    default: true,
    tests: &[
        case!(paths_stay_in_sandbox),
        case!(config_only_udp_goes_to_node),
        case!(config_keeps_dialer_chain),
        case!(config_refuses_nodes_without_udp),
        case!(pick_node),
        case!(state_and_set),
        case!(helper_version_change_is_stale),
        case!(build_files),
        // 本机 UDP 走节点只在 macOS 上有
        case!(view_and_running).needs(&[Need::MacOs]),
        case!(env_and_leak_hint).needs(&[Need::MacOs]),
    ],
};

/// root 那边的启动脚本、launchd 配置、规则的版本（cli/src/cmd/corenet/udp.rs 的同名常量）
const UDP_HELPER: i64 = 2;

fn vless() -> Value {
    obj! {
        "name" => "US-A", "type" => "vless", "server" => "192.0.2.10", "port" => 443, "uuid" => "00000000-0000-4000-8000-000000000000",
        "udp" => true, "tls" => true, "servername" => "example.com", "flow" => "xtls-rprx-vision",
        "reality-opts" => obj! { "public-key" => "secret-public-key", "short-id" => "abcd" },
    }
}

fn noudp() -> Value {
    obj! { "name" => "US-B", "type" => "trojan", "server" => "192.0.2.11", "port" => 443, "password" => "pw-b" }
}

fn hy2() -> Value {
    obj! { "name" => "HK-A", "type" => "hysteria2", "server" => "192.0.2.12", "port" => 8443, "password" => "pw-h" }
}

fn chain() -> Value {
    obj! {
        "name" => "US-C", "type" => "ss", "server" => "192.0.2.13", "port" => 8388, "cipher" => "aes-128-gcm", "password" => "pw-c",
        "udp" => true, "dialer-proxy" => "HK-A",
    }
}

/// 沙箱里的假命令。ps：gui-state 看在跑的进程（照 ps-command 这个文件回答）；route：内置代理查公网的流量走哪块网卡
/// （答物理网卡，不用绑定）。别的会动到这台机器、会联网的命令一律是只会报错的替身，调到了收尾时算失败
const FAKES: &[(&str, &str)] = &[
    ("ps", "case \"$*\" in *command*) cat \"$HOME/ps-command\" 2>/dev/null; exit 0 ;; esac\n"),
    ("route", "case \"$*\" in \"-n get \"*) echo \"  interface: en0\"; exit 0 ;; esac\n"),
];
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
    "netstat",
    "ipconfig",
];

struct Fx {
    sb: Sandbox,
}

impl Fx {
    fn new(t: &mut Ctx) -> Fx {
        let mut sb = Sandbox::new(t, "cca-udp-");
        std::fs::create_dir_all(sb.cca.join("core")).unwrap();
        std::fs::create_dir(sb.bin()).unwrap();
        sb.guard(t, GUARDED, FAKES);
        let (cca, path) = (sb.cca.clone(), format!("{}:/usr/bin:/bin", sb.bin().display()));
        sb.set_env("CCA_HOME", &cca).set_env("CCA_TOOL", "claude").set_env("PATH", path);
        let fx = Fx { sb };
        fx.nodes(&[vless(), noudp(), hy2(), chain()]);
        fx.config(obj! { "client" => "builtin" });
        fx
    }

    /// 内置代理拷来的节点（~/.cca/core/nodes.json）
    fn nodes(&self, proxies: &[Value]) {
        let names: Vec<Value> = proxies.iter().map(|p| p["name"].clone()).collect();
        let doc = obj! { "saved_at" => 1, "nodes" => names, "proxies" => proxies.to_vec() };
        fsx::write(self.sb.cca.join("core/nodes.json"), json::dumps(&doc, &Style::default()));
    }

    /// config.json：local 那一节和可选节点
    fn config(&self, local: Value) {
        self.config_with(local, &["US-A", "US-B", "HK-A", "US-C"]);
    }

    fn config_with(&self, local: Value, nodes: &[&str]) {
        let doc = obj! { "version" => 2, "nodes" => nodes.to_vec(), "local" => local };
        fsx::write(self.sb.cca.join("config.json"), json::dumps(&doc, &Style::default()));
    }

    fn cfg(&self) -> Value {
        fsx::read_json(self.sb.cca.join("config.json"))
    }

    /// 跑一个子命令（stdin 是 /dev/null）；返回 (stdout, stderr, 退出码)。辅助程序自己出了错（内部错误、没处理的错误）
    /// 是在 stderr 上说 cca-core:，不是哪个子命令该有的退出
    fn run_cmd(&self, args: &[&str], env: &[(&str, &str)]) -> (String, String, i32) {
        let mut cmd = self.sb.helper(args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let r = ran(cmd.timeout(Duration::from_secs(30)));
        assert!(!r.err.contains("cca-core:"), "{}", r.err);
        (r.out, r.err, r.code)
    }

    /// bash 的 udp_env 拿到的：root 那边的位置、设的节点和状态、装着的是哪个节点
    fn udp_env(&self) -> BTreeMap<String, String> {
        let (out, err, code) = self.run_cmd(&["udp-env"], &[]);
        assert_eq!((code, err.as_str()), (0, ""));
        shell_vars(&out)
    }

    /// (设的节点, 状态)
    fn udp_state(&self) -> (String, String) {
        let v = self.udp_env();
        (v["UDP_NODE"].clone(), v["UDP_STATE"].clone())
    }

    /// root 那边的目录（沙箱里换进了 HOME 下面）
    fn udp_root(&self) -> String {
        self.udp_env()["UDP_ROOT"].clone()
    }

    /// root 那边的 TUN 内核的配置（udp-build 写出来的 config.yaml）；这个节点没有定义、不转发 UDP 时不肯写，是 None
    fn udp_config(&self, name: &str) -> Option<String> {
        let folder = sandbox::mkdtemp(&self.sb.home, "config-");
        fsx::write(folder.join("cca-udp"), "#!/bin/sh\n");
        let core = self.sb.home.join("mihomo");
        fsx::write(&core, b"fake core");
        let (out, err, code) =
            self.run_cmd(&["udp-build", name, &folder.display().to_string(), &core.display().to_string(), "1.19.33"], &[]);
        if code != 0 {
            let want = format!("内置代理里没有节点 {name} 转发 UDP 的定义\n");
            assert_eq!((code, out.as_str(), err.as_str()), (1, "", want.as_str()));
            let names: Vec<String> =
                std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            assert_eq!(names, ["cca-udp"]);
            return None;
        }
        Some(fsx::read_text(folder.join("config.yaml")))
    }

    /// 界面看的那一份（_gui state 的 config.local.udp；bash 是带着 CCA_RO=1 跑它的）。procs 是 ps 看到的进程 [(进程号, 命令行)]
    fn udp_view(&self, procs: &[(i32, String)]) -> Value {
        let text: String = procs.iter().map(|(pid, command)| format!("{pid:5} {command}\n")).collect();
        fsx::write(self.sb.home.join("ps-command"), text);
        let (out, err, code) = self.run_cmd(&["gui-state"], &[("CCA_RO", "1")]);
        assert_eq!((code, err.as_str()), (0, ""));
        let state = json::parse(&out).unwrap();
        assert_eq!(state["errors"], list![]);
        state["config"]["local"]["udp"].clone()
    }

    /// WebRTC 直连风险那一句警告后面的补充：给 leak-verdict 一份 STUN 看到中国大陆地址的探测结果（归属地都带着，不用再查）
    fn udp_leak_hint(&self) -> String {
        let now = now();
        let geo = obj! {
            "203.0.113.0/24" => obj! { "t" => now, "cc" => "us", "country" => "United States", "isp" => "Example ISP" },
            "192.0.2.0/24" => obj! { "t" => now, "cc" => "cn", "country" => "China", "isp" => "Example ISP" },
        };
        let path = self.sb.home.join("probe.json");
        let probe = obj! {
            "dns" => obj! { "ok" => true, "servers" => list!["203.0.113.53"], "geo" => geo },
            "stun" => obj! { "ips" => list!["192.0.2.77"] },
        };
        fsx::write(&path, json::dumps(&probe, &Style::default()));
        let (out, err, code) =
            self.run_cmd(&["leak-verdict", &path.display().to_string(), "local|US-A", "203.0.113.10", "US", "detail"], &[]);
        assert_eq!((code, err.as_str()), (0, ""));
        let warnings: Vec<&str> = sandbox::splitlines(&out)
            .into_iter()
            .filter(|row| row.starts_with("W\twebrtc\t"))
            .map(|row| row.splitn(3, '\t').nth(2).unwrap())
            .collect();
        assert_eq!(warnings.len(), 1, "{out}");
        let (said, hint) = match warnings[0].split_once("并限制绕过账号代理的 UDP 流量") {
            Some((said, hint)) => (said, Some(hint)),
            None => (warnings[0], None),
        };
        assert!(said.starts_with("WebRTC 直连风险：") && hint.is_some(), "{}", warnings[0]);
        hint.unwrap().to_string()
    }

    /// 模仿 root 那边装好了：udp-build 写出来的 info.json 放进沙箱里的 root 目录
    fn install(&self, name: &str) -> (PathBuf, BTreeMap<String, String>) {
        let folder = self.sb.home.join(format!("build-{name}"));
        std::fs::create_dir(&folder).unwrap();
        fsx::write(folder.join("cca-udp"), "#!/bin/sh\n");
        let core = self.sb.home.join("mihomo");
        fsx::write(&core, b"fake core");
        let (out, _, code) =
            self.run_cmd(&["udp-build", name, &folder.display().to_string(), &core.display().to_string(), "1.19.33"], &[]);
        assert_eq!(code, 0);
        let root = PathBuf::from(self.udp_root());
        std::fs::create_dir_all(&root).unwrap();
        fsx::write(root.join("info.json"), fsx::read(folder.join("info.json")));
        (folder, shell_vars(&out))
    }
}

/// 现在（1970 年以来的秒，带小数：`time.time()`）
fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

/// 一个网段（`ipaddress.ip_network`，strict：主机位不是 0 的不认）：IPv6 不是、第一个地址、前缀长度。
/// 只认 `地址/前缀长度` 和光一个地址（整个前缀）；写成网络掩码的（Python 也认）这里不认，遇到就失败——辅助程序没那样写过
#[derive(Clone, Copy, Debug, PartialEq)]
struct Net {
    v6: bool,
    start: u128,
    prefix: u32,
}

/// 一个地址（`ipaddress.ip_address`）：(IPv6 不是, 数值)
fn ip(text: &str) -> (bool, u128) {
    if let Ok(a) = text.parse::<std::net::Ipv4Addr>() {
        return (false, u128::from(u32::from(a)));
    }
    match text.parse::<std::net::Ipv6Addr>() {
        Ok(a) => (true, u128::from(a)),
        Err(_) => panic!("不是 IP 地址：{text:?}"),
    }
}

impl Net {
    fn parse(text: &str) -> Net {
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let (v6, start) = ip(addr);
        let max = if v6 { 128 } else { 32 };
        let prefix = match prefix {
            None => max,
            Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                p.parse::<u32>().ok().filter(|n| *n <= max).unwrap_or_else(|| panic!("前缀长度不对：{text:?}"))
            }
            Some(_) => panic!("不认识的网段写法：{text:?}"),
        };
        let net = Net { v6, start, prefix };
        assert_eq!(start & net.host_mask(), 0, "{text:?} 的主机位不是 0");
        net
    }

    fn host_mask(&self) -> u128 {
        let host_bits = if self.v6 { 128 } else { 32 } - self.prefix;
        if host_bits == 128 {
            u128::MAX
        } else {
            (1u128 << host_bits) - 1
        }
    }

    /// 最后一个地址（broadcast_address）
    fn end(&self) -> u128 {
        self.start | self.host_mask()
    }

    /// 这个地址在不在里面（版本不同的不在）
    fn contains(&self, (v6, addr): (bool, u128)) -> bool {
        v6 == self.v6 && addr & !self.host_mask() == self.start
    }

    /// 两个网段有没有交叠（照 Python 的 overlaps：一个的头或尾在另一个里面）
    fn overlaps(&self, other: &Net) -> bool {
        other.contains((self.v6, self.start))
            || other.contains((self.v6, self.end()))
            || self.contains((other.v6, other.start))
            || self.contains((other.v6, other.end()))
    }
}

fn paths_stay_in_sandbox(t: &mut Ctx) {
    let fx = Fx::new(t);
    let v = fx.udp_env();
    let (root, plist, log, owner) = (&v["UDP_ROOT"], &v["UDP_PLIST"], &v["UDP_LOG"], &v["UDP_OWNER"]);
    for p in [root, plist, log] {
        assert!(p.starts_with(&fx.sb.home.display().to_string()), "{p}");
    }
    assert!(root.ends_with("/cca-udp"), "{root}");
    // SAFETY: getuid、getgid 只是问一下
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    assert_eq!(owner, &format!("{uid}:{gid}"));
    assert_eq!(v["UDP_FLAG"], fx.sb.cca.join("udp/on").display().to_string());
}

fn config_only_udp_goes_to_node(t: &mut Ctx) {
    let fx = Fx::new(t);
    let c = json::parse(&fx.udp_config("US-A").expect("US-A 应该写得出配置")).unwrap();
    let rules = c["rules"].list();
    assert_eq!(rules[0], Value::from("NETWORK,TCP,DIRECT"));
    assert_eq!(rules[1], Value::from("DST-PORT,53,DIRECT"));
    assert!(rules[2].str().contains("PROCESS-NAME-REGEX"), "{}", rules[2]);
    assert_eq!(rules[rules.len().saturating_sub(2)..], [Value::from("NETWORK,UDP,US-A"), Value::from("MATCH,DIRECT")]);
    let tun = &c["tun"];
    assert!(
        sandbox::truthy(&tun["enable"]) && sandbox::truthy(&tun["auto-route"]) && sandbox::truthy(&tun["auto-detect-interface"]),
        "{tun}"
    );
    assert_eq!(tun["dns-hijack"], list![]);
    assert_eq!(tun["device"], Value::from("utun233")); // 网卡名固定：root 那边的启动脚本靠它分清路由表里哪些是自己的
    assert!(tun.get("route-exclude-address").is_none(), "{tun}"); // 只给 route-address：sing-tun 照原样加，不再合并成和别的 TUN 一样大的段
    let routes: Vec<Net> = tun["route-address"].list().iter().map(|r| Net::parse(r.str())).collect();
    for net in [
        "192.168.0.0/16",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "198.18.0.0/15",
        "224.0.0.0/4",
        "fe80::/10",
        "fc00::/7",
        "ff00::/8",
        "::1/128",
    ] {
        let net_ = Net::parse(net);
        assert!(!routes.iter().any(|r| r.overlaps(&net_)), "{net}"); // 局域网、保留地址不进 TUN
    }
    for addr in ["1.1.1.1", "8.8.8.8", "203.0.113.10", "223.5.5.5", "198.20.0.1", "2001:4860:4860::8888", "2001:db8::1"] {
        assert_eq!(routes.iter().filter(|r| r.contains(ip(addr))).count(), 1, "{addr}");
        // 公网地址正好在一段里
    }
    for big in ["0.0.0.0/1", "128.0.0.0/1", "1.0.0.0/8", "64.0.0.0/2", "::/1", "8000::/1", "2000::/3"] {
        assert!(!routes.contains(&Net::parse(big)), "{big}"); // 和别的 TUN 加的大段一样大的一条都没有
    }
    assert_eq!(c["dns"], obj! { "fake-ip-range" => "198.19.233.233/30" }); // 只给 TUN 的地址（不开 DNS）
    let names: Vec<Value> = c["proxies"].list().iter().map(|p| p["name"].clone()).collect();
    assert_eq!(names, [Value::from("US-A")]);
    for k in ["external-controller", "external-controller-unix", "port", "mixed-port", "socks-port", "listeners"] {
        assert!(c.get(k).is_none(), "{k}");
    }
    assert!(!sandbox::truthy(&c["allow-lan"]), "{c}");
}

fn config_keeps_dialer_chain(t: &mut Ctx) {
    let fx = Fx::new(t);
    let c = json::parse(&fx.udp_config("US-C").expect("US-C 应该写得出配置")).unwrap();
    let mut names: Vec<String> = c["proxies"].list().iter().map(|p| p["name"].str().to_string()).collect();
    names.sort();
    assert_eq!(names, ["HK-A", "US-C"]);
}

fn config_refuses_nodes_without_udp(t: &mut Ctx) {
    let fx = Fx::new(t);
    assert_eq!(fx.udp_config("US-B"), None); // trojan 没写 udp: true
    assert!(fx.udp_config("HK-A").is_some()); // hysteria2 本来就转发 UDP
    assert_eq!(fx.udp_config("NOPE"), None);
    assert_eq!(fx.udp_config("US-A,DIRECT"), None);
}

fn pick_node(t: &mut Ctx) {
    let fx = Fx::new(t);
    let (out, _, code) = fx.run_cmd(&["udp-node", "US-A"], &[]);
    assert_eq!((out.as_str(), code), ("US-A\n", 0));
    let (out, _, code) = fx.run_cmd(&["udp-node", "US-B"], &[]);
    assert_eq!(code, 3);
    assert_eq!(out.split_whitespace().collect::<Vec<_>>(), ["US-A", "HK-A", "US-C"]);
    let (_, _, code) = fx.run_cmd(&["udp-node", "NOPE"], &[]);
    assert_eq!(code, 2);
    let (out, _, code) = fx.run_cmd(&["udp-node"], &[]);
    assert_eq!(code, 2);
    assert!(!out.split_whitespace().any(|n| n == "US-B"), "{out}");
}

fn state_and_set(t: &mut Ctx) {
    let fx = Fx::new(t);
    let state = |node: &str, state: &str| (node.to_string(), state.to_string());
    assert_eq!(fx.udp_state(), state("", "off"));
    fx.run_cmd(&["local-set", "udp", "US-A"], &[]);
    assert_eq!(fx.cfg()["local"], obj! { "client" => "builtin", "udp" => "US-A" });
    assert_eq!(fx.udp_state(), state("US-A", "missing"));
    fx.install("US-A");
    assert_eq!(fx.udp_state(), state("US-A", "ok"));
    // 路由器上改了这个节点：装着的配置旧了
    let mut moved = vless().obj().clone();
    moved.insert("server", "192.0.2.99");
    fx.nodes(&[Value::Obj(moved), noudp(), hy2(), chain()]);
    assert_eq!(fx.udp_state(), state("US-A", "stale"));
    fx.nodes(&[noudp(), hy2(), chain()]); // 节点定义没了
    assert_eq!(fx.udp_state(), state("US-A", "undefined"));
    fx.nodes(&[vless(), noudp(), hy2(), chain()]);
    fx.run_cmd(&["local-set", "udp", ""], &[]); // 关掉：这一项去掉
    assert_eq!(fx.cfg()["local"], obj! { "client" => "builtin" });
    assert_eq!(fx.udp_state(), state("", "off"));
    fx.run_cmd(&["local-set", "udp", "US-A"], &[]);
    fx.run_cmd(&["local-set", "client", "", "core_off", "1"], &[]); // cca local off：换掉内置代理时一起清掉
    assert_eq!(fx.cfg()["local"], obj! { "core_off" => "1" });
    fx.config(obj! { "client" => "", "udp" => "US-A" }); // 不是内置代理：不算开着
    assert_eq!(fx.udp_state(), state("", "off"));
}

fn helper_version_change_is_stale(t: &mut Ctx) {
    let fx = Fx::new(t);
    fx.run_cmd(&["local-set", "udp", "US-A"], &[]);
    fx.install("US-A");
    assert_eq!(fx.udp_state(), ("US-A".to_string(), "ok".to_string()));
    // 辅助程序里的版本从外面改不了，改装着的那一份记的：比现在的旧（升级了 cca）、比现在的新（退回了旧版）都要重新装
    let info = PathBuf::from(fx.udp_root()).join("info.json");
    let installed = fsx::read_json(&info);
    assert_eq!(installed["helper"], Value::from(UDP_HELPER));
    for helper in [UDP_HELPER - 1, UDP_HELPER + 1] {
        let mut changed = installed.obj().clone();
        changed.insert("helper", helper);
        fsx::write(&info, json::dumps(&Value::Obj(changed), &Style::default()));
        assert_eq!(fx.udp_state(), ("US-A".to_string(), "stale".to_string()));
    }
}

fn build_files(t: &mut Ctx) {
    let fx = Fx::new(t);
    let (folder, v) = fx.install("US-A");
    let cfg = folder.join("config.yaml");
    assert_eq!(fsx::mode(&cfg) & 0o777, 0o600);
    assert_eq!(v["B_CFG"], crypto::sha256_hex(&fsx::read(&cfg)));
    assert_eq!(v["B_PLIST"], crypto::sha256_hex(&fsx::read(folder.join("plist"))));
    assert_eq!(v["B_INFO"], crypto::sha256_hex(&fsx::read(folder.join("info.json"))));
    assert_eq!(v["B_BIN"], crypto::sha256_hex(b"fake core"));
    assert_eq!(v["B_SAME"], "");
    let p = plist::load(&fsx::read(folder.join("plist"))).unwrap();
    let root = fx.udp_root();
    let flag = fx.sb.cca.join("udp/on").display().to_string();
    assert_eq!(p.get("Label"), Some(&plist::Value::Str("local.cca.udp".to_string())));
    assert_eq!(p.get("ProgramArguments"), Some(&plist::Value::Array(vec![plist::Value::Str(format!("{root}/cca-udp"))])));
    let keep_alive =
        plist::Value::Dict(vec![("PathState".to_string(), plist::Value::Dict(vec![(flag, plist::Value::Bool(true))]))]);
    assert_eq!(p.get("KeepAlive"), Some(&keep_alive));
    let info = fsx::read_json(folder.join("info.json"));
    assert_eq!(list![&info["node"], &info["sum"], &info["helper"]], list!["US-A", v["B_CFG"].as_str(), UDP_HELPER]);
    for f in ["plist", "info.json"] {
        // 谁都能读的文件里没有节点密钥
        assert!(!String::from_utf8_lossy(&fsx::read(folder.join(f))).contains("secret-public-key"), "{f}");
    }
    assert!(fsx::read_text(&cfg).contains("secret-public-key"));
    // 再生成一次同样的：B_SAME
    let again = fx.sb.home.join("again");
    std::fs::create_dir(&again).unwrap();
    fsx::write(again.join("cca-udp"), "#!/bin/sh\n");
    let core = fx.sb.home.join("mihomo").display().to_string();
    let (out, _, code) = fx.run_cmd(&["udp-build", "US-A", &again.display().to_string(), &core, "1.19.33"], &[]);
    assert_eq!((code, shell_vars(&out)["B_SAME"].as_str()), (0, "1"));
    let (_, err, code) = fx.run_cmd(&["udp-build", "US-B", &again.display().to_string(), &core, "1.19.33"], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("没有节点 US-B 转发 UDP 的定义"), "{err}");
}

/// ps 看到的进程表
fn procs(list: &[(i32, &str)]) -> Vec<(i32, String)> {
    list.iter().map(|(pid, command)| (*pid, command.to_string())).collect()
}

/// 界面看的那一份应该是的样子
fn view(node: Option<&str>, state: &str, installed: bool, running: bool, other_vpn: Option<&str>, paused: bool) -> Value {
    obj! {
        "node" => node, "state" => state, "installed" => installed, "running" => running, "other_vpn" => other_vpn,
        "paused" => paused,
    }
}

fn view_and_running(t: &mut Ctx) {
    let fx = Fx::new(t);
    assert_eq!(fx.udp_view(&[]), view(None, "off", false, false, None, false));
    fx.run_cmd(&["local-set", "udp", "US-A"], &[]);
    fx.install("US-A");
    let root = fx.udp_root();
    let running = procs(&[(10, &format!("{root}/mihomo -d {root} -f {root}/config.yaml"))]);
    assert_eq!(fx.udp_view(&running), view(Some("US-A"), "ok", true, true, None, false));
    // 在不在跑只认命令行里「root 那边的目录/mihomo -d 」这一段
    assert!(sandbox::truthy(&fx.udp_view(&procs(&[(11, &format!("/bin/bash {root}/mihomo -d x"))]))["running"]));
    assert!(!sandbox::truthy(&fx.udp_view(&procs(&[(12, &format!("/usr/bin/vi {root}/mihomo"))]))["running"]));
    // 启动脚本给别的 VPN 让开了：other-vpn 里的网卡名，启动脚本在跑时才算；只认网卡名
    fsx::write(Path::new(&root).join("other-vpn"), "utun5\n");
    let script = procs(&[(13, &format!("/bin/sh {root}/cca-udp"))]);
    assert_eq!(fx.udp_view(&script), view(Some("US-A"), "ok", true, false, Some("utun5"), false));
    assert!(fx.udp_view(&running)["other_vpn"].is_null());
    fsx::write(Path::new(&root).join("other-vpn"), "utun5; rm -rf /\n");
    assert!(fx.udp_view(&script)["other_vpn"].is_null());
    // 在家暂停着：开关文件不在、留着在家暂停的记号（~/.cca/udp/home），而且装着的就是设的这一份；开关文件在、没设、旧了都不算
    std::fs::create_dir_all(fx.sb.cca.join("udp")).unwrap();
    fsx::write(fx.sb.cca.join("udp/home"), "");
    assert_eq!(fx.udp_view(&[]), view(Some("US-A"), "ok", true, false, None, true));
    fsx::write(fx.sb.cca.join("udp/on"), "");
    assert!(!sandbox::truthy(&fx.udp_view(&[])["paused"]));
    std::fs::remove_file(fx.sb.cca.join("udp/on")).unwrap();
    fx.run_cmd(&["local-set", "udp", ""], &[]);
    assert_eq!(fx.udp_view(&[]), view(None, "off", true, false, None, false));
}

fn env_and_leak_hint(t: &mut Ctx) {
    let fx = Fx::new(t);
    let (out, _, code) = fx.run_cmd(&["udp-env"], &[]);
    let v = shell_vars(&out);
    assert_eq!((code, v["UDP_STATE"].as_str(), v["UDP_NODE"].as_str(), v["UDP_INSTALLED"].as_str()), (0, "off", "", ""));
    assert_eq!((v["UDP_TUN"].as_str(), v["UDP_OTHER"].as_str()), ("utun233", ""));
    let hint = fx.udp_leak_hint();
    assert!(hint.contains("cca local udp on"), "{hint}");
    fx.run_cmd(&["local-set", "udp", "US-A"], &[]);
    let hint = fx.udp_leak_hint();
    assert!(hint.contains("cca local udp 查看"), "{hint}");
    fx.install("US-A"); // 装好了、启动脚本给别的 VPN 让开了：说 UDP 照它的规则出去
    let root = PathBuf::from(fx.udp_root());
    fsx::write(root.join("other-vpn"), "utun5\n");
    fsx::write(fx.sb.home.join("ps-command"), format!("   13 /bin/sh {}/cca-udp\n", root.display()));
    assert_eq!(fx.udp_env()["UDP_OTHER"], "utun5");
    let hint = fx.udp_leak_hint();
    assert!(hint.contains("另一个 VPN（utun5）接管了这台 Mac 的流量"), "{hint}");
    fsx::write(fx.sb.home.join("ps-command"), "");
    let hint = fx.udp_leak_hint();
    assert!(hint.contains("cca local udp 查看"), "{hint}");
    fx.config(obj! {});
    assert_eq!(fx.udp_leak_hint(), "");
}
