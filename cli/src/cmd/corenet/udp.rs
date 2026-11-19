//! 本机 UDP 走节点（cca local udp，只在 macOS 上；bash 的同名一节）：`udp_paths` 到 `cmd_udp_probe`。
//!
//! 内置代理的内核是普通用户的进程，只在 127.0.0.1 上开账号的端口，碰不到这台 Mac 自己的 UDP：WebRTC、QUIC 这些直连出去，STUN 看到
//! 的是本地网络的出口（在国内就是中国大陆的地址）。macOS 上开 TUN 要 root，所以另装一个 root 的系统服务（LaunchDaemon local.cca.udp）：
//! /Library/Application Support/cca-udp 里一份 root 自己的 mihomo 和配置，只开 TUN，UDP 交给选定的节点，TCP 照旧直连，DNS（53 端口）、
//! 局域网、ZeroTier / Tailscale、代理内核自己的流量不经过它。launchd 在 ~/.cca/udp/on 存在时起它（KeepAlive 的 PathState，开机后也是），
//! 开关文件没了它自己停下、退出：开关不用管理员密码；装、换节点、节点定义变了、卸载要输一次。root 那边的文件只在输管理员密码的那一步
//! 从私有的临时目录拷过去、按摘要逐个核对后才换上：普通用户的进程改不了它跑什么，只能开关

use super::{client_builtin, core_defs, core_pick, def_name, install_names, install_resolve, sect, type_name};
use crate::cmd::dock::home_swapped;
use crate::cmd::leak::{geo_fresh, ip_prefix, leak_cache, leak_geo, leak_where, stun_ips};
use crate::cmd::session::gui_procs;
use crate::cmd::{arg, arity};
use crate::core::base::{sh, CCA_HOME, CFG};
use crate::core::model::cfg_load;
use crate::py::fs::load_quiet;
use crate::py::hash::{sha256_hex, Sha256Stream};
use crate::py::json;
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::path;
use crate::py::plist::{self, P};
use crate::py::proc::getenv;
use crate::py::time::time;
use crate::py::{die, exit, R};
use crate::{dict, list, out, re};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::rc::Rc;
use std::sync::LazyLock;

pub const UDP_LABEL: &str = "local.cca.udp";
/// root 那边的启动脚本（bash 的 udp_run_sh）、launchd 的配置、规则的版本：改了要加 1（装着的旧版显示要重新装）
pub const UDP_HELPER: i64 = 1;
pub static UDP_FLAG: LazyLock<String> = LazyLock::new(|| path::join_all(&[&CCA_HOME, "udp", "on"]));
/// 不进 TUN：局域网、组播、链路本地、CGNAT（Tailscale）
const UDP_EXCLUDE: &[&str] = &[
    "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16",
    "224.0.0.0/4", "240.0.0.0/4", "fc00::/7", "fe80::/10", "ff00::/8",
];
/// 这些进程的 UDP 照旧直连：虚拟局域网打洞、别的代理内核（cca 自己的内核开着 TUN 时本来就绑定物理网卡，这里再保一次）
const UDP_DIRECT_PROCS: &str = "^(zerotier-one|tailscaled|mihomo.*|verge-mihomo.*)$";
/// 不写 udp: true 也转发 UDP 的节点类型
const UDP_ALWAYS: &[&str] = &["hysteria", "hysteria2", "tuic", "wireguard"];

/// root 那边的位置：(目录, launchd 配置, 日志, 文件的属主)。测试的沙箱（home_swapped）里换进沙箱，碰不到真的 /Library
pub fn udp_paths() -> (String, String, String, String) {
    if home_swapped() {
        let base = path::join(&path::expanduser("~"), "udp-root");
        let or = |k: &str, default: String| getenv(k).filter(|v| !v.is_empty()).unwrap_or(default);
        // SAFETY: getuid、getgid 只是问一下，不会失败
        let owner = unsafe { format!("{}:{}", libc::getuid(), libc::getgid()) };
        return (
            or("CCA_UDP_ROOT", path::join(&base, "cca-udp")),
            or("CCA_UDP_PLIST", path::join(&base, &format!("{UDP_LABEL}.plist"))),
            or("CCA_UDP_LOG", path::join(&base, "cca-udp.log")),
            owner,
        );
    }
    (
        "/Library/Application Support/cca-udp".to_string(),
        format!("/Library/LaunchDaemons/{UDP_LABEL}.plist"),
        "/Library/Logs/cca-udp.log".to_string(),
        "root:wheel".to_string(),
    )
}

/// 能写进规则的节点名（规则用逗号分字段）
fn udp_name_ok(n: &str) -> bool {
    !n.is_empty() && !re!(r"[,\x00-\x1f\x7f]").found(n)
}

/// 这个节点转发 UDP 吗（mihomo 里 ss、vmess、vless、trojan 这些要写 udp: true；http、ssh 不转发）
fn udp_capable(p: Option<&Dict>) -> bool {
    let Some(p) = p else {
        return false;
    };
    let kind = p.str("type");
    let kind_in = |names: &[&str]| kind.as_deref().is_some_and(|k| names.contains(&k));
    kind_in(UDP_ALWAYS) || (matches!(p.get("udp"), Some(V::Bool(true))) && !kind_in(&["http", "ssh"]))
}

/// root 那边的 TUN 内核的配置（JSON 写的，mihomo 照 YAML 读），节点没有定义、不转发 UDP 时是 None。只放这个节点（和它经
/// dialer-proxy 用到的）：别的节点变了不用重新装。没有入站、控制接口，不劫持 DNS（照系统的走）。规则：TCP 直连；DNS 直连；
/// 虚拟局域网、别的代理内核的 UDP 直连；其余 UDP 交给节点
fn udp_config(name: &str) -> Option<String> {
    let defs = core_pick(&core_defs(), &[V::from(name)]);
    if !udp_name_ok(name) || !udp_capable(defs.iter().find(|p| &*def_name(p) == name)) {
        return None;
    }
    let cfg = dict! {
        "mode" => "rule", "log-level" => "warning", "ipv6" => true, "allow-lan" => false, "find-process-mode" => "strict",
        "geo-auto-update" => false, "profile" => dict! { "store-selected" => false, "store-fake-ip" => false },
        "tun" => dict! { "enable" => true, "stack" => "mixed", "auto-route" => true, "auto-detect-interface" => true, "strict-route" => false,
                         "dns-hijack" => List::new(), "route-exclude-address" => str_list(UDP_EXCLUDE) },
        "proxies" => List::from_vec(defs.into_iter().map(V::Dict).collect()),
        "rules" => list!["NETWORK,TCP,DIRECT", "DST-PORT,53,DIRECT", format!("PROCESS-NAME-REGEX,{UDP_DIRECT_PROCS},DIRECT"),
                         format!("NETWORK,UDP,{name}"), "MATCH,DIRECT"],
    };
    Some(json::dumps(&V::Dict(cfg), json::Fmt::new().utf8().indent(1)) + "\n")
}

/// root 那边装着的（info.json：节点、配置的摘要……，谁都能读、没有密钥）；没装、读不了时是 None
fn udp_info() -> Option<Dict> {
    match load_quiet(&path::join(&udp_paths().0, "info.json")) {
        Some(V::Dict(j)) if j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) => Some(j),
        _ => None,
    }
}

/// 本机 UDP 走节点：(设的节点, 状态)。状态：off（没开）| ok | stale（装着的配置和现在算出来的不一样：节点定义变了、cca 改了
/// 模板，要重新装）| missing（root 的系统服务没装：被删掉了）| undefined（内置代理里已经没有这个节点转发 UDP 的定义）
pub fn udp_state(c: &Dict) -> (String, &'static str) {
    let loc = sect(c, "local");
    let node = if matches!(loc.get("client"), Some(V::Str(s)) if &*s == "builtin") { loc.get("udp") } else { None };
    let Some(node) = node.and_then(|n| n.rc_str()).filter(|n| !n.is_empty()) else {
        return (String::new(), "off");
    };
    let node = node.to_string();
    let Some(text) = udp_config(&node) else {
        return (node, "undefined");
    };
    let Some(info) = udp_info() else {
        return (node, "missing");
    };
    let same = |k: &str, want: V| info.get(k).is_some_and(|v| v.py_eq(&want));
    if !same("helper", V::Int(UDP_HELPER)) || !same("node", V::from(&node)) || !same("sum", V::from(sha256_hex(text))) {
        return (node, "stale");
    }
    (node, "ok")
}

/// root 那边的 TUN 内核在跑吗（普通用户的 ps 也看得到 root 进程的命令行；和 bash 的 udp_running 一样认）。
/// procs 是 `gui_procs()` 的结果，没给就现查
pub fn udp_running(procs: Option<&[(i64, String)]>) -> bool {
    let mark = format!("{}/mihomo -d ", udp_paths().0);
    match procs {
        Some(rows) => rows.iter().any(|(_, cmd)| cmd.contains(&mark)),
        None => gui_procs().iter().any(|(_, cmd)| cmd.contains(&mark)),
    }
}

/// _gui state 的 config.local.udp（PROTOCOL 2.2）：只在 macOS 上，别的系统是 None
pub fn udp_view(c: &Dict, procs: Option<&[(i64, String)]>) -> V {
    if !cfg!(target_os = "macos") {
        return V::None;
    }
    let (node, state) = udp_state(c);
    let installed = udp_info().is_some();
    let running = !node.is_empty() && udp_running(procs);
    let node = if node.is_empty() { V::None } else { V::from(node) };
    V::Dict(dict! { "node" => node, "state" => state, "installed" => installed, "running" => running })
}

/// WebRTC 直连风险的补充：这台 Mac 用内置代理时，怎么让它自己的 UDP 也经节点出去（cca local udp）
pub fn udp_leak_hint() -> String {
    if !cfg!(target_os = "macos") {
        return String::new();
    }
    let Some(V::Dict(c)) = load_quiet(&CFG) else {
        return String::new();
    };
    if !client_builtin(&c) {
        return String::new();
    }
    let (node, state) = udp_state(&c);
    if state == "off" {
        return "。用内置代理时，cca local udp on 节点 能让这台 Mac 的 UDP 也经节点出去（要输一次管理员密码）".to_string();
    }
    format!("。设了本机 UDP 走节点（{node}），现在没起作用：cca local udp 查看")
}

/// bash 的 udp_env：root 那边的位置、设的节点和状态、装着的是哪个节点
pub fn cmd_udp_env(args: &[String]) -> R<()> {
    arity("cmd_udp_env", args, 0, 0)?;
    let c = cfg_load()?;
    let (root, plist, log, owner) = udp_paths();
    let (node, state) = udp_state(&c);
    let installed = udp_info().and_then(|info| info.str("node")).unwrap_or_else(|| Rc::from(""));
    sh(&[
        ("UDP_ROOT", &root),
        ("UDP_PLIST", &plist),
        ("UDP_LOG", &log),
        ("UDP_OWNER", &owner),
        ("UDP_LABEL", UDP_LABEL),
        ("UDP_FLAG", &UDP_FLAG),
        ("UDP_NODE", &node),
        ("UDP_STATE", state),
        ("UDP_INSTALLED", &installed),
    ]);
    Ok(())
}

/// 这台 Mac 的 UDP 交给哪个节点：want 在这台机器的节点定义里叫什么（同名、改过名的、按服务器地址认，和辅助安装一样）。
/// 认不出时退出码 2，认出了、它不转发 UDP 时 3；这两种 stdout 都是能选的节点（转发 UDP 的，一行一个）
pub fn cmd_udp_node(args: &[String]) -> R<()> {
    arity("cmd_udp_node", args, 0, 1)?;
    let want = arg(args, 0, "");
    let c = cfg_load()?;
    // 同名的以第一个为准（和 core_pick 一样）
    let mut defs: Vec<(Rc<str>, Dict)> = Vec::new();
    for p in core_defs() {
        let n = def_name(&p);
        if !defs.iter().any(|(have, _)| *have == n) {
            defs.push((n, p));
        }
    }
    let capable = |n: &str| udp_name_ok(n) && udp_capable(defs.iter().find(|(have, _)| &**have == n).map(|(_, p)| p));
    let got = if want.is_empty() { String::new() } else { install_resolve(&c, want) };
    if !got.is_empty() && capable(&got) {
        out!("{got}");
        return Ok(());
    }
    for n in install_names() {
        if capable(&n) {
            out!("{n}");
        }
    }
    exit(if got.is_empty() { 2 } else { 3 })
}

/// 把要装到 root 那边的文件写进私有的临时目录 folder（bash 已经写好启动脚本 cca-udp）：config.yaml（600，有节点密钥）、plist、
/// info.json；输出它们和内核文件的摘要（bash 写进用管理员权限运行的脚本，root 拷完逐个核对）。B_SAME=1：装着的就是这一份
pub fn cmd_udp_build(args: &[String]) -> R<()> {
    arity("cmd_udp_build", args, 4, 4)?;
    let (name, folder, core_bin, core_version) = (args[0].as_str(), args[1].as_str(), args[2].as_str(), args[3].as_str());
    let Some(text) = udp_config(name) else {
        return die(format!("内置代理里没有节点 {name} 转发 UDP 的定义"));
    };
    let root = udp_paths().0;
    // 新建（已经有了是错），写进去，返回内容的摘要
    let put = |file: &str, data: &[u8], mode: u32| -> R<String> {
        std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path::join(folder, file))?.write_all(data)?;
        Ok(sha256_hex(data))
    };
    let file_sum = |p: &str| -> R<String> {
        let mut f = std::fs::File::open(p)?;
        let (mut h, mut buf) = (Sha256Stream::new(), vec![0u8; 1 << 20]);
        loop {
            match f.read(&mut buf) {
                Ok(0) => return Ok(h.hexdigest()),
                Ok(n) => h.update(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    };
    let s_cfg = put("config.yaml", text.as_bytes(), 0o600)?;
    let launchd = P::dict([
        ("Label", P::Str(UDP_LABEL.to_string())),
        ("ProgramArguments", P::Array(vec![P::Str(format!("{root}/cca-udp"))])),
        ("KeepAlive", P::dict([("PathState", P::dict([(UDP_FLAG.as_str(), P::Bool(true))]))])),
        ("ThrottleInterval", P::Int(5)),
    ]);
    let s_plist = put("plist", &plist::dumps_xml(&launchd)?, 0o644)?;
    let s_run = file_sum(&path::join(folder, "cca-udp"))?;
    let s_bin = file_sum(core_bin)?;
    let info = dict! {
        "v" => 1, "helper" => UDP_HELPER, "node" => name, "sum" => &s_cfg, "core" => core_version, "bin" => &s_bin, "run" => &s_run,
        "plist" => &s_plist, "flag" => UDP_FLAG.as_str(), "saved_at" => time().trunc() as i64,
    };
    let old = udp_info().unwrap_or_default();
    let same = info.items().iter().filter(|(k, _)| &**k != "saved_at").all(|(k, v)| old.get(k).is_some_and(|o| o.py_eq(v)));
    let s_info = put("info.json", (json::dumps(&V::Dict(info), json::Fmt::new().utf8().indent(1)) + "\n").as_bytes(), 0o644)?;
    sh(&[("B_CFG", &s_cfg), ("B_PLIST", &s_plist), ("B_RUN", &s_run), ("B_BIN", &s_bin), ("B_INFO", &s_info), ("B_SAME", if same { "1" } else { "" })]);
    Ok(())
}

/// 这台 Mac 的 UDP 现在从哪出去：STUN 看到的地址、国家代码、归属地（直连查），一行一个，没有的项写 -；一个都没测到时退出码 1
pub fn cmd_udp_probe(args: &[String]) -> R<()> {
    arity("cmd_udp_probe", args, 0, 0)?;
    let ips = stun_ips(1.5);
    if ips.is_empty() {
        return exit(1);
    }
    let now = time();
    // (leak_cache().get('geo') or {}).items()
    let cached = match leak_cache().get("geo") {
        Some(V::Dict(geo)) => geo,
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'items'", type_name(&v)),
        _ => Dict::new(),
    };
    let known = Dict::from_pairs(cached.items().into_iter().filter(|(_, g)| geo_fresh(g, now)));
    let geo = leak_geo(&ips, "", &known, 0.0);
    for ip in &ips {
        let g = geo.get(&ip_prefix(ip)).filter(|g| g.truthy());
        let cc = match &g {
            Some(V::Dict(g)) => g.get("cc").filter(|cc| cc.truthy()).map(|cc| cc.py_str()),
            Some(other) => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(other)),
            None => None,
        };
        let place = leak_where(g.as_ref());
        out!("{}\t{}\t{}", ip, cc.as_deref().unwrap_or("-"), if place.is_empty() { "-" } else { &place });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_capability() {
        // tests/local_udp.py 的节点：vless 写了 udp: true、trojan 没写、hysteria2 本来就转发
        assert!(udp_name_ok("US-A") && udp_name_ok("节点 1") && !udp_name_ok("") && !udp_name_ok("US-A,DIRECT") && !udp_name_ok("a\tb") && !udp_name_ok("a\u{7f}"));
        assert!(!udp_name_ok("a\n") && udp_name_ok("a\u{80}"));
        let cap = |p: Dict| udp_capable(Some(&p));
        assert!(cap(dict! { "type" => "vless", "udp" => true }) && !cap(dict! { "type" => "trojan" }) && cap(dict! { "type" => "hysteria2" }));
        assert!(cap(dict! { "type" => "hysteria" }) && cap(dict! { "type" => "tuic" }) && cap(dict! { "type" => "wireguard", "udp" => false }));
        assert!(!cap(dict! { "type" => "http", "udp" => true }) && !cap(dict! { "type" => "ssh", "udp" => true }) && cap(dict! { "type" => "socks5", "udp" => true }));
        // udp 要正好是 true：1、"true" 都不算；没有 type 的只看 udp
        assert!(!cap(dict! { "type" => "ss", "udp" => 1 }) && !cap(dict! { "type" => "ss", "udp" => "true" }) && cap(dict! { "udp" => true }));
        assert!(cap(dict! { "type" => 5, "udp" => true }) && !cap(dict! { "type" => list!["tuic"] }) && !cap(Dict::new()) && !udp_capable(None));
    }

    #[test]
    fn running_is_read_from_the_command_line() {
        // tests/local_udp.py 的 test_view_and_running：只认「root 那边的目录/mihomo -d 」这一段
        let root = udp_paths().0;
        let rows = |cmd: String| vec![(10, cmd)];
        assert!(udp_running(Some(&rows(format!("{root}/mihomo -d {root} -f {root}/config.yaml")))));
        assert!(udp_running(Some(&rows(format!("/bin/bash {root}/mihomo -d x")))));
        assert!(!udp_running(Some(&rows(format!("/usr/bin/vi {root}/mihomo")))));
        assert!(!udp_running(Some(&rows(format!("{root}/mihomo -t -d {root}")))));
        assert!(!udp_running(Some(&[])));
    }

    #[test]
    fn off_without_a_node() {
        // 没设节点、不是内置代理时不看节点定义，直接是 off（tests/local_udp.py 的 test_state_and_set 的头尾）
        assert_eq!(udp_state(&dict! { "local" => dict! { "client" => "builtin" } }), (String::new(), "off"));
        assert_eq!(udp_state(&dict! { "local" => dict! { "client" => "", "udp" => "US-A" } }), (String::new(), "off"));
        assert_eq!(udp_state(&dict! { "local" => dict! { "client" => "builtin", "udp" => "" } }), (String::new(), "off"));
        assert_eq!(udp_state(&dict! { "local" => dict! { "client" => "builtin", "udp" => 5 } }), (String::new(), "off"));
        assert_eq!(udp_state(&dict! { "local" => V::None }), (String::new(), "off"));
        assert_eq!(udp_state(&Dict::new()), (String::new(), "off"));
    }

    #[test]
    fn view_without_a_node() {
        // tests/local_udp.py 的 test_view_and_running 开头：没设节点时 node 是 None、不看进程（installed 看 root 那边装没装，这里不管）。
        // 设了节点的几种状态要有节点定义，在对拍里（udp-env、local-env 的 udp- 用例看的是同一个 udp_state）
        let rows = vec![(10, format!("{}/mihomo -d x", udp_paths().0))];
        let view = udp_view(&dict! { "local" => dict! { "client" => "builtin" } }, Some(&rows));
        if cfg!(target_os = "macos") {
            let view = view.as_dict().unwrap();
            assert!(view.at("node").is_none() && view.at("state").as_str() == Some("off") && view.at("running").as_bool() == Some(false));
            assert!(view.at("installed").is_bool());
            assert_eq!(view.keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["node", "state", "installed", "running"]);
        } else {
            assert!(view.is_none());
        }
    }
}
