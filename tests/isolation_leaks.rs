//! 泄露检测的证据边界：临时 HOME、cca 的辅助程序（cca-core leak-verdict、leak-probe）、模拟的探测结果，不联网
//! （原来的 tests/isolation_leaks.py）。
//!
//! 只能在进程内测的一处在辅助程序自己的单元测试里（cargo test）：STUN 连不上、不回答时拿不到 IP
//! （cli/src/cmd/leak/stun.rs 的 failures_are_empty_never_an_address）。

use kit::case;
use kit::fsx;
use kit::harness::{self, Ctx, Module};
use kit::json::{self, Value};
use kit::obj;
use kit::proc::{Cmd, Output};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MODULE: Module = Module {
    name: "isolation_leaks",
    src: include_str!("isolation_leaks.rs"),
    default: true,
    tests: &[
        case!(empty_or_missing_stun_result_is_failed),
        case!(webrtc_off_on_servers_is_not_mentioned),
        case!(empty_dns_response_is_failed_and_cached_briefly),
        case!(missing_dns_geo_is_failed),
        case!(known_foreign_resolver_only_establishes_region_check),
        case!(known_cn_resolver_is_still_reported_with_an_unknown_resolver),
        case!(mainland_stun_ip_is_risk_not_browser_measurement),
        case!(other_regions_are_only_a_note),
        case!(any_mainland_address_is_a_risk),
        case!(unknown_location_is_neither_a_warning_nor_clear),
        case!(stun_without_ip_cannot_produce_ok),
        case!(dns_polling_without_observed_resolvers_does_not_succeed),
        case!(disabled_check_emits_no_claim),
    ],
};

const LEAK_FAIL_TTL: i64 = 600; // 没测成的过 10 分钟再测（cli/src/cmd/leak.rs 的同名常量）
const EXIT_IP: &str = "203.0.113.10";
const RESOLVER: &str = "203.0.113.53";
const KEY: &str = "router|US-A";

/// 一个用例的沙箱：临时 HOME，里面有 .cca 和一个只会失败的 curl
struct Fx {
    home: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    let home = t.tmp("cca-leaks-");
    std::fs::create_dir(home.join(".cca")).unwrap();
    // 查归属地的 curl 一律拿不到（退出码 7）：结论只能来自探测结果里带着的
    fsx::write_script(home.join("bin/curl"), "#!/bin/sh\necho \"$@\" >> \"$HOME/curl.log\"\nexit 7\n");
    Fx { home }
}

/// 现在的时间（秒，带小数：time.time()）
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("时钟在 1970 年以后").as_secs_f64()
}

/// 归属地按网段记：IPv4 的 /24（辅助程序的 ip_prefix）
fn ip_prefix(ip: &str) -> String {
    format!("{}.0/24", ip.split('.').take(3).collect::<Vec<_>>().join("."))
}

/// 一条查到的归属地
fn geo(cc: &str) -> Value {
    let country = match cc {
        "us" => "United States",
        "cn" => "China",
        "ca" => "Canada",
        _ => "",
    };
    obj! { "t" => now(), "cc" => cc, "country" => country, "isp" => "Example ISP" }
}

/// DNS 的探测结果：看到了哪几个解析器（没给就是 RESOLVER 一个），每个都带着归属地 cc
fn dns(cc: &str, servers: Option<&[&str]>) -> Value {
    let servers = servers.unwrap_or(&[RESOLVER]);
    let mut geos = json::Obj::new();
    for ip in servers {
        geos.insert(ip_prefix(ip), geo(cc));
    }
    obj! { "ok" => true, "servers" => servers.to_vec(), "geo" => geos }
}

impl Fx {
    fn helper(&self, args: &[&str]) -> Output {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(args)
            .env("HOME", &self.home)
            .env("CCA_HOME", self.home.join(".cca"))
            .env("CCA_TOOL", "claude")
            .env("PATH", format!("{}:/usr/bin:/bin", self.home.join("bin").display()))
            .timeout(Duration::from_secs(30));
        kit::bash::run(&mut cmd)
    }

    /// leak-verdict 对这份探测结果说的话，和其中 S 行给的各项状态（项 → 状态）
    fn verdict(&self, payload: &Value) -> (String, BTreeMap<String, String>) {
        let path = self.home.join("probe.json");
        fsx::write(&path, json::dumps(payload, &json::Style::default()));
        let r = self.helper(&["leak-verdict", &path.display().to_string(), KEY, EXIT_IP, "US", "detail"]);
        assert_eq!(r.status.py(), 0, "{}", r.err());
        let text = r.out();
        let mut states = BTreeMap::new();
        for row in text.lines().filter(|row| row.starts_with("S\t")) {
            let cells: Vec<&str> = row.split('\t').collect();
            states.insert(cells[1].to_string(), cells[2].to_string());
        }
        (text, states)
    }

    /// STUN 看到这几个地址（各自的归属地是 cc），DNS 照常
    fn stun(&self, seen: &[(&str, &str)]) -> (String, BTreeMap<String, String>) {
        let mut dns = dns("us", None);
        for (ip, cc) in seen {
            dns.as_obj_mut().unwrap().get_mut("geo").unwrap().as_obj_mut().unwrap().insert(ip_prefix(ip), geo(cc));
        }
        let ips: Vec<&str> = seen.iter().map(|(ip, _)| *ip).collect();
        self.verdict(&obj! { "dns" => dns, "stun" => obj! { "ips" => ips } })
    }
}

fn states(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn empty_or_missing_stun_result_is_failed(t: &mut Ctx) {
    let fx = fixture(t);
    let no_ips: Vec<&str> = Vec::new();
    for payload in
        [obj! { "dns" => dns("us", None), "stun" => obj! { "ips" => no_ips.clone() } }, obj! { "dns" => dns("us", None) }, obj! {}]
    {
        t.sub(format!("payload={}", payload.compact()), |_| {
            let (text, states) = fx.verdict(&payload);
            assert_eq!(states["webrtc"], "failed");
            assert!(text.contains("STUN 探测未取得 IP"), "{text}");
            assert!(!text.contains("无泄露"), "{text}");
        });
    }
}

/// 服务器上（没有账号专属的浏览器和桌面 App）leak-probe 写明 webrtc: off：不测也不说，不是“没测成”
fn webrtc_off_on_servers_is_not_mentioned(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, states) = fx.verdict(&obj! { "dns" => dns("us", None), "webrtc" => "off" });
    assert!(!states.contains_key("webrtc"), "{states:?}");
    assert_eq!(states["dns"], "ok");
    assert!(!text.contains("WebRTC"), "{text}");
}

fn empty_dns_response_is_failed_and_cached_briefly(t: &mut Ctx) {
    let fx = fixture(t);
    let no_ips: Vec<&str> = Vec::new();
    let (text, states) = fx.verdict(&obj! { "dns" => dns("us", Some(&[])), "stun" => obj! { "ips" => no_ips } });
    assert_eq!(states["dns"], "failed");
    assert!(text.contains("未取得解析器"), "{text}");
    let cached = fsx::read_json(fx.home.join(".cca/leak.json"))["dns"][KEY].clone();
    assert!(!cached["ok"].bool());
    // 没测成的只记 10 分钟：刚记下的还新（时间就是现在），过了 LEAK_FAIL_TTL 就不算数（leak_dns_fresh 的边界在
    // cli/src/cmd/leak.rs 的单元测试 freshness 里）
    assert!((now() - cached["t"].f64()).abs() < 30.0, "{cached}");
    assert_eq!(LEAK_FAIL_TTL, 600);
}

fn missing_dns_geo_is_failed(t: &mut Ctx) {
    let fx = fixture(t);
    for cc in ["", "unknown", "??"] {
        t.sub(format!("cc={cc:?}"), |_| {
            let (text, states) = fx.verdict(&obj! { "dns" => dns(cc, None), "stun" => obj! { "ips" => vec![EXIT_IP] } });
            assert_eq!(states["dns"], "failed");
            assert!(text.contains("解析器归属地不完整"), "{text}");
        });
    }
}

fn known_foreign_resolver_only_establishes_region_check(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, got) = fx.verdict(&obj! { "dns" => dns("ca", None), "stun" => obj! { "ips" => vec![EXIT_IP] } });
    assert_eq!(got, states(&[("dns", "ok"), ("webrtc", "ok")]));
    assert!(text.contains("DNS 未发现地区异常"), "{text}");
    assert!(text.contains("非浏览器实测"), "{text}");
    assert!(text.contains("STUN 出口一致（浏览器状态未验证）"), "{text}");
    assert!(!text.contains("无泄露"), "{text}");
}

fn known_cn_resolver_is_still_reported_with_an_unknown_resolver(t: &mut Ctx) {
    let fx = fixture(t);
    let mut payload = dns("cn", None);
    let Some(Value::List(servers)) = payload.as_obj_mut().unwrap().get_mut("servers") else { panic!("servers 是数组") };
    servers.push(Value::from("192.0.2.53"));
    let (text, states) = fx.verdict(&obj! { "dns" => payload, "stun" => obj! { "ips" => vec![EXIT_IP] } });
    assert_eq!(states["dns"], "leak");
    assert!(text.contains("DNS 泄露（1 个解析器在中国大陆）"), "{text}");
}

fn mainland_stun_ip_is_risk_not_browser_measurement(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, states) = fx.stun(&[("192.0.2.77", "cn")]);
    assert_eq!(states["webrtc"], "leak");
    assert!(text.contains("W\twebrtc\t"), "{text}");
    assert!(text.contains("WebRTC 直连风险：192.0.2.77"), "{text}");
    assert!(text.contains("尚未验证浏览器或 Claude App 是否会暴露该 IP"), "{text}");
    assert!(!text.contains("都拿得到"), "{text}");
    assert!(!text.contains("会暴露 192.0.2.77"), "{text}");
}

fn other_regions_are_only_a_note(t: &mut Ctx) {
    let fx = fixture(t);
    for cc in ["us", "hk", "mo", "tw"] {
        t.sub(format!("cc={cc:?}"), |_| {
            let (text, states) = fx.stun(&[("192.0.2.77", cc)]);
            assert_eq!(states["webrtc"], "note");
            assert!(!text.contains("W\twebrtc"), "{text}");
            assert!(text.contains("N\twebrtc\t"), "{text}");
            assert!(text.contains("WebRTC 备注：STUN 看到 192.0.2.77"), "{text}");
            assert!(!text.contains("直连风险"), "{text}");
        });
    }
}

fn any_mainland_address_is_a_risk(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, states) = fx.stun(&[("192.0.2.77", "us"), ("198.51.100.77", "cn")]);
    assert_eq!(states["webrtc"], "leak");
    assert!(text.contains("WebRTC 直连风险：198.51.100.77"), "{text}");
}

fn unknown_location_is_neither_a_warning_nor_clear(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, states) = fx.verdict(&obj! { "dns" => dns("us", None), "stun" => obj! { "ips" => vec!["192.0.2.77"] } });
    assert_eq!(states["webrtc"], "failed");
    assert!(!text.contains("W\twebrtc"), "{text}");
    assert!(text.contains("归属地没查到"), "{text}");
}

/// STUN 连不上、不回答时探测结果里没有 IP（辅助程序的单元测试）；没有 IP 的结果不能判成通过
fn stun_without_ip_cannot_produce_ok(t: &mut Ctx) {
    let fx = fixture(t);
    let no_ips: Vec<&str> = Vec::new();
    let (_, states) = fx.verdict(&obj! { "stun" => obj! { "ips" => no_ips } });
    assert_eq!(states["webrtc"], "failed");
}

/// 查 DNS 的服务一直说没看到解析器（假 curl 每次都回空的列表）：问满几轮以后是没测成，不是通过
fn dns_polling_without_observed_resolvers_does_not_succeed(t: &mut Ctx) {
    let fx = fixture(t);
    fsx::write(
        fx.home.join("bin/curl"),
        r#"#!/bin/sh
echo "$@" >> "$HOME/curl.log"
case "$*" in */api/dns/result/*) echo '{"dns_servers": []}' ;; esac
exit 0
"#,
    );
    let r = fx.helper(&["leak-probe", "http://127.0.0.1:7901", KEY, "", "0"]);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    let result = json::parse(&r.out()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{}", r.out()));
    assert_eq!((result["dns"]["ok"].bool(), result["dns"]["servers"].list()), (false, &[][..]));
    assert!(fsx::read_text(fx.home.join("curl.log")).matches("/api/dns/result/").count() >= 2);
}

fn disabled_check_emits_no_claim(t: &mut Ctx) {
    let fx = fixture(t);
    let (text, states) = fx.verdict(&obj! { "off" => true });
    assert_eq!(text, "");
    assert_eq!(states, BTreeMap::new());
}
