//! iOS App 的协议核心（ios/CcaRemoteKit，Swift）：没有 Xcode 也测得了——用命令行工具的 swiftc 编成 macOS 上的程序跑
//! （docs/dev/ios.md「测试」）。
//! - against_the_spec：kit::spec（照 server/README.md 独立写的那一份）出数据，Swift 那边逐项核对（ios/Tests/KitTests）；
//!   Swift 封的配对消息、发的记录、签名再拿回这边核对（签名用 ssh-keygen -Y verify）
//! - pair_list_attach_type、wrong_pin_and_used_code、session_ends、pushes_reach_the_phone：整条路。box 上是真的 cca（辅助程序：在宿主里运行的会话、
//!   remote-pair-show --web），手机那一头是 ios/tools/ccaguest（和 App 用的是同一份 CcaRemoteKit：钉住证书的 HTTPS、TCP 进数据
//!   端口）：扫二维码（链接）配对、列会话、接入、敲键、只看、取消配对；登记推送，会话里的通知经服务器、推送网关（kit::apnsfake 的假网关、
//!   假推送服务）送到，像通知扩展那样拿通知密钥解开
//! - accounts_reach_the_phone：手机上的「账号」页和额度的通知：列会话时带回 box 上的账号（box 的辅助程序 remote-status 放的），额度用完、
//!   恢复的通知经服务器推到，字在手机上写；登记推送时不带 usage 就不推
//! - in_the_simulator：模拟器里真的 App（要 Xcode；先跑 `bash ios/test.sh`，再带 CCA_IOS_SIM=1）：配对、接入、敲键、通知、滚动，
//!   登记推送、通知密钥在共用的钥匙串分组里、推来的那一份解得开
//!
//! 没有 swiftc（不是 macOS）时跳过。编出来的程序放在 ios/build/test/（源码没变时用上次的）。夹具（Base、服务器）在 kit::relay 里。
//! 原来是 tests/ios_kit.py：数据由 spec_wire.py 出，这里换成 kit::spec，核对的一样。

use kit::apnsfake::{Gateway, Knobs};
use kit::fleet::{self, Device};
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module, Need};
use kit::json::{self, Style, Value};
use kit::proc::{Child, Cmd, Stdio};
use kit::relay::{self, has, secs, Base, Popen};
use kit::spec::{self, Channel};
use kit::{case, crypto, list, obj, pki};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SWIFT: &[Need] = &[Need::MacOs, Need::Bin("swiftc")];

pub const MODULE: Module = Module {
    name: "ios_kit",
    src: include_str!("ios_kit.rs"),
    default: true,
    tests: &[
        // 头一次要编（几十秒）
        case!(against_the_spec).needs(SWIFT).timeout(600),
        case!(pair_list_attach_type).needs(SWIFT).timeout(600),
        case!(wrong_pin_and_used_code).needs(SWIFT).timeout(600),
        case!(session_ends).needs(SWIFT).timeout(600),
        case!(a_phone_launches_a_session).needs(SWIFT).timeout(600),
        case!(pushes_reach_the_phone).needs(SWIFT).timeout(600),
        case!(accounts_reach_the_phone).needs(SWIFT).timeout(600),
        case!(in_the_simulator).needs(&[Need::MacOs]).timeout(600),
    ],
};

// ---- 编 Swift 的程序 ----

/// 路径当参数用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

/// 目录里的 .swift，按名字排
fn swift_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读 {}：{e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "swift"))
        .collect();
    files.sort();
    files
}

/// CcaRemoteKit 的源码加上 main（一个目录）编成一个程序；源码没变时用上次的（ios/build/test/<名字>-<源码的摘要>）。回程序的路径
fn build(name: &str, main: &Path) -> PathBuf {
    let ios = harness::repo().join("ios");
    let kit = ios.join("CcaRemoteKit").join("Sources").join("CcaRemoteKit");
    let srcs: Vec<PathBuf> = swift_files(&kit).into_iter().chain(swift_files(main)).collect();
    let mut all = Vec::new();
    for p in &srcs {
        all.extend_from_slice(p.file_name().and_then(|n| n.to_str()).expect("文件名").as_bytes());
        all.push(0);
        all.extend_from_slice(&fsx::read(p));
    }
    let cache = ios.join("build").join("test");
    let out = cache.join(format!("{name}-{}", &crypto::hex(&crypto::sha256(&all))[..16]));
    if out.exists() {
        return out;
    }
    std::fs::create_dir_all(&cache).unwrap_or_else(|e| panic!("建 {}：{e}", cache.display()));
    for old in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
        if old.file_name().to_str().is_some_and(|n| n.starts_with(&format!("{name}-"))) {
            let _ = std::fs::remove_file(old.path());
        }
    }
    let tmp = cache.join(format!("{name}.tmp{}", std::process::id()));
    let arch = if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x86_64" };
    let mut cmd = Cmd::new("swiftc");
    cmd.args(["-Onone", "-swift-version", "5", "-module-name", &name.replace('-', "_"), "-target"]);
    cmd.arg(format!("{arch}-apple-macos14.0")).arg("-o").arg(&tmp).args(&srcs);
    cmd.env_inherit().timeout(secs(500));
    let r = cmd.output().unwrap_or_else(|e| panic!("起不了 swiftc：{e}"));
    if !r.status.success() {
        let err = r.err();
        panic!("编不过：\n{}", &err[err.len().saturating_sub(4000)..]);
    }
    std::fs::rename(&tmp, &out).unwrap_or_else(|e| panic!("{e}"));
    out
}

// ---- 对照 kit::spec ----

/// 0、7、14……（每个字节是下标乘 7 的低 8 位）
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7) as u8).collect()
}

/// 证书（PEM）换成 DER
fn der_of(cert: &Path) -> Vec<u8> {
    let mut cmd = Cmd::new("openssl");
    cmd.args(["x509", "-in"]).arg(cert).args(["-outform", "der"]).pass_env(&["PATH"]);
    cmd.output().unwrap_or_else(|e| panic!("{e}")).check().stdout
}

/// 网址里的一项：字母、数字和 `-._~` 原样，别的（UTF-8 的每个字节）写成 %XX（Python 的 `quote(s, safe='-._~')`）
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn against_the_spec(t: &mut Ctx) {
    let exe = build("kit-tests", &harness::repo().join("ios").join("Tests").join("KitTests"));
    let d = t.tmp("cca-kit-");
    let hx = |b: &[u8]| crypto::hex(b);
    let code_row = |c: &[u8]| obj! { "hex" => hx(c), "text" => spec::code_text(c), "rid" => spec::pair_keys(c).2 };
    let codes: Vec<Vec<u8>> = vec![crypto::random(10), crypto::random(10), crypto::random(10), vec![0; 10], vec![0xff; 10]];
    let mut v = json::Obj::new();
    let rows: Vec<Value> = codes[..2].iter().chain(&codes[3..]).map(|c| code_row(c)).collect();
    v.insert("codes", Value::List(rows));
    v.insert(
        "bad_codes",
        list![
            "",
            "ABCD-EFGH-JKMN-PQR",
            "ABCD-EFGH-JKMN-PQRST",
            "ABCD-EFGH-JKMN-PQRU",
            "ABCD-EFGH-JKMN-PQR*",
            "ＡBCD-EFGH-JKMN-PQRS"
        ],
    );
    // 配对
    let code = &codes[2];
    let (kc, keys, rid) = spec::pair_keys(code);
    let (na, nb) = (crypto::random(32), crypto::random(32));
    let (fpa, fpb) = (format!("SHA256:{}xyz", "a/b+".repeat(10)), format!("SHA256:{}", "Q".repeat(43)));
    let (p, pid) = spec::pair_secret(&kc, &na, &nb, &fpa, &fpb);
    v.insert(
        "pair",
        obj! {
            "code" => hx(code), "other_code" => crypto::token_hex(10),
            "msg_a" => spec::pair_msg(&keys, &rid, "a", &fpa, &na, "Mac 上的 cca"),
            "fp_a" => &fpa, "n_a" => hx(&na), "name_a" => "Mac 上的 cca", "nb" => hx(&nb), "fp_b" => &fpb, "p" => hx(&p), "pid" => &pid
        },
    );
    // 会话的说明
    let sid = crypto::token_hex(16);
    let cwd = format!("~/srv/{}", "x".repeat(300));
    let doc = obj! {
        "v" => 1, "machine" => "box\u{202e}\u{1b}", "tool" => "claude", "name" => "账号 1", "cwd" => &cwd, "since" => 1763800000.5,
        "access" => "view"
    };
    v.insert(
        "dir",
        obj! {
            "p" => hx(&p), "pid" => &pid, "sid" => &sid, "blob" => spec::dir_seal(&p, &pid, &sid, &doc),
            "forged" => spec::dir_seal(&crypto::random(32), &pid, &sid, &doc),
            "control" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "since" => true }),
            // 那一头结束得了它（远程启动的会话、能操作的配对）；只让看的写了也不算，不是 true 的（字、数）不算
            "ends" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "access" => "control", "end" => true }),
            "ends_view" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "access" => "view", "end" => true }),
            "ends_odd" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "end" => "yes" }),
            "ends_one" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "end" => 1 }),
            "want" => obj! {
                "machine" => "box", "tool" => "claude", "name" => "账号 1", "cwd" => &cwd[..200], "since" => 1763800000.5, "view" => true
            }
        },
    );
    // 推送的通知（辅助程序的 note_seal 封的那种）：标题、副标题、正文清理过、限长度；别的配对的、别的密钥封的、样子不对的都不认
    let (nid, nsid) = (crypto::token_hex(8), crypto::token_hex(16));
    let long = format!("改好了\n两处{}", "字".repeat(300));
    let note = |patch: &[(&str, Value)]| {
        let mut doc = obj! {
            "v" => 1, "id" => &nid, "sid" => &nsid, "at" => 1763800000, "title" => "做完了\u{1b}[31m", "subtitle" => "alice · work\u{202e}",
            "body" => &long
        };
        let o = doc.as_obj_mut().expect("对象");
        for (k, x) in patch {
            if x.is_null() {
                o.remove(k);
            } else {
                o.insert(*k, x.clone());
            }
        }
        doc
    };
    let body: String = format!("改好了两处{}", "字".repeat(300)).chars().take(240).collect();
    v.insert(
        "note",
        obj! {
            "p" => hx(&p), "pid" => &pid, "key" => hx(&crypto::hmac_sha256(&p, b"cca-remote/v1/note")),
            "other_pid" => crypto::token_hex(16), "other_p" => hx(&crypto::random(32)),
            "blob" => spec::note_seal(&p, &pid, &note(&[])),
            "forged" => spec::note_seal(&crypto::random(32), &pid, &note(&[])),
            "no_id" => spec::note_seal(&p, &pid, &note(&[("id", Value::Null)])),
            "bad_id" => spec::note_seal(&p, &pid, &note(&[("id", Value::from("0123456789ABCDEF"))])),
            "bad_sid" => spec::note_seal(&p, &pid, &note(&[("sid", Value::from("xyz"))])),
            "empty" => spec::note_seal(&p, &pid, &note(&[("title", Value::from("")), ("body", Value::from(" \u{1b}"))])),
            "v2" => spec::note_seal(&p, &pid, &note(&[("v", Value::from(2))])),
            "dir" => spec::dir_seal(&p, &pid, &nsid, &note(&[])),
            "want" => obj! {
                "id" => &nid, "sid" => &nsid, "at" => 1763800000, "title" => "做完了[31m", "subtitle" => "alice · work", "body" => &body
            }
        },
    );
    // 账号的状态（辅助程序的 status_seal 封的那种，3.7.5 起）：读成模型；额度用完的规则和辅助程序 status.rs 的 exhausted 一样（下面这几种
    // 情形和那里的 what_counts_as_used_up、antigravity_counts_the_least_used_group 是同样的）
    let now = 1_794_000_000.0; // UTC 21:20
    let patched = |mut doc: Value, patch: &[(&str, Value)]| {
        let o = doc.as_obj_mut().expect("对象");
        for (k, x) in patch {
            if x.is_null() {
                o.remove(k);
            } else {
                o.insert(*k, x.clone());
            }
        }
        doc
    };
    let win = |u: f64, r: f64| obj! { "utilization" => u, "resets_epoch" => r, "reset" => false };
    let usage = |five: Value, week: Value| {
        obj! {
            "source" => "live", "fetched_epoch" => now - 60.0, "stale" => false, "error_code" => Value::Null, "five_hour" => five,
            "seven_day" => week, "model_scoped" => Value::List(vec![]), "use_first" => Value::Null
        }
    };
    let row = |id: char, name: &str, tool: &str, login: &str, u: Value| {
        obj! {
            "id" => id.to_string().repeat(16), "tool" => tool, "name" => name, "kind" => "oauth", "login" => login, "plan" => "Max",
            "tier" => "premium", "busy" => false, "usage" => u, "weeks" => Value::Null
        }
    };
    let bucket = |slot: &str, u: f64, r: f64| {
        obj! { "utilization" => u, "resets_epoch" => r, "reset" => false, "slot" => slot, "disabled" => false, "remaining_amount" => Value::Null }
    };
    let alice_usage = patched(
        usage(win(100.0, now + 3600.0), win(41.0, now + 3.0 * 86400.0)),
        &[(
            "model_scoped",
            list![obj! {
                "name" => "Fable", "utilization" => 18.0, "resets_epoch" => now + 3.0 * 86400.0, "reset" => false,
                "allowance_percent_of_weekly" => 50.0, "used_percent_of_weekly" => 9.0
            }],
        )],
    );
    let week = |start: f64, used: f64, current: bool| {
        obj! {
            "start_epoch" => start, "end_epoch" => start + 7.0 * 86400.0, "used_percent" => used, "resets" => 0.0, "current" => current,
            "partial" => false
        }
    };
    // 折合金额（status.rs 的 cost_of 给的样子）：按周的有一项是 120 小时（中间换过切法），开始有记录的时间在第二项里面；按模型、
    // 按设备的各有一项特别的（没名字的是没按模型记下的那一部分，暂按别的模型的价格估算的带第四项；少几项的设备不要）
    let (week_end, day_end) = (now + 3.0 * 86400.0, now + 2.0 * 3600.0);
    let cost = obj! {
        "since" => week_end - 432.0 * 3600.0, "weeks" => list![0, 1250, 4800, 483], "week_end" => week_end,
        "week_hours" => list![168, 168, 120, 168], "days" => list![120, 0, 377], "day_end" => day_end, "sum" => list![6533, 497],
        "average" => list![4800, Value::Null], "average_n" => list![1, 0], "cache_hit" => list![0.973, Value::Null], "sync" => "error",
        "missing" => 2,
        "models" => list![
            list!["claude-opus-5-5", 5000, 400],
            list![Value::Null, 1533, 97],
            list!["claude-haiku-5-6", 0, 0, "claude-haiku-5-5"]
        ],
        "devices" => list![
            list!["studio-mac", 1, 5000, 400, 0, now - 60.0],
            list!["office-imac\u{202e}", 0, 1533, 97, 1, Value::Null],
            list!["short", 1]
        ]
    };
    let alice = patched(
        row('a', "alice", "claude", "logged_in", alice_usage),
        &[
            ("app", Value::from(true)),
            ("cli", Value::from(2)),
            ("since", Value::from(now - 7200.5)),
            ("cost", cost),
            ("aid", Value::from("ab".repeat(16))),
            ("color", Value::from("#7C3AED")),
            ("badge", Value::from("AL")),
            ("busy", Value::from(true)),
            (
                "weeks",
                obj! {
                    "weeks" => list![week(now - 4.0 * 86400.0, 41.0, true), week(now - 11.0 * 86400.0, 63.0, false)],
                    "summary" => obj! { "weeks" => 1.0, "average_percent" => 63.0, "maxed" => 0.0, "under_half" => 0.0 }
                },
            ),
        ],
    );
    let bob_usage = patched(usage(win(100.0, now + 60.0), Value::Null), &[("error_code", Value::from("auth"))]);
    let bob = row('b', "bob", "codex", "logged_in", bob_usage);
    let carol = row('c', "carol", "claude", "logged_out", usage(win(100.0, now + 60.0), Value::Null));
    let dave =
        row('d', "dave", "claude", "logged_in", usage(win(99.6, now + 600.0), obj! { "utilization" => 100.0, "reset" => true }));
    let gem_usage = patched(
        usage(Value::Null, Value::Null),
        &[(
            "quota_groups",
            list![
                obj! { "name" => "Gemini Models", "buckets" => list![bucket("five_hour", 100.0, now + 100.0), bucket("seven_day", 100.0, now + 5000.0)] },
                obj! {
                    "name" => "Claude and GPT models",
                    "buckets" => list![bucket("five_hour", 30.0, now + 200.0), bucket("seven_day", 100.0, now + 4000.0)]
                }
            ],
        )],
    );
    let gem = row('e', "gem", "agy", "logged_in", gem_usage);
    let fay = row(
        'f',
        "fay",
        "claude",
        "logged_in",
        patched(usage(win(100.0, now + 60.0), Value::Null), &[("source", Value::from("none"))]),
    );
    let gus = row('1', "gus", "claude", "logged_in", usage(win(100.0, now - 5.0), Value::Null));
    let hal = row('2', "hal", "claude", "logged_in", usage(win(92.0, now + 60.0), Value::Null));
    let ivy =
        row('3', "ivy", "claude", "logged_in", patched(usage(win(95.0, now + 60.0), Value::Null), &[("stale", Value::from(true))]));
    let bad = patched(row('9', "bad", "claude", "logged_in", Value::Null), &[("id", Value::from("XYZ"))]);
    // 那台电脑的折合金额那几样（cost_meta，账号太多时带 brief）、这个配对所在的 Team（team_of）
    let sdoc = obj! {
        "v" => 1, "machine" => "box\u{202e}", "more" => 3, "at" => now - 30.0,
        "accounts" => list![alice, bob, carol, dave, gem, bad, fay, gus, hal, ivy],
        "cost" => obj! { "prices_date" => "2026-11-01", "updated" => now - 240.0, "unattributed" => 312, "brief" => true },
        "team" => obj! {
            "name" => "示例 Team\u{202e}", "role" => "admin", "coadmin" => true, "state" => "error", "error" => "连不上服务器", "seq" => 12,
            "accounts" => 3, "device" => "studio-mac", "host" => "team.example.com:18445", "joined" => now - 86400.0,
            "update" => "服务器不认 status_put"
        }
    };
    // 放不下时：不带折合金额、每周用量（"cost": false、"weeks": false），旧版的宿主不给 Team
    let lean = patched(sdoc.clone(), &[("cost", Value::from(false)), ("weeks", Value::from(false)), ("team", Value::Null)]);
    let id = |c: char| c.to_string().repeat(16);
    v.insert(
        "status",
        obj! {
            "p" => hx(&p), "pid" => &pid, "now" => now, "key" => hx(&crypto::hmac_sha256(&p, b"cca-remote/v1/status")),
            "other_pid" => crypto::token_hex(16),
            "blob" => spec::status_seal(&p, &pid, &sdoc),
            "forged" => spec::status_seal(&crypto::random(32), &pid, &sdoc),
            "dir" => spec::dir_seal(&p, &pid, &nsid, &sdoc),
            "note" => spec::note_seal(&p, &pid, &sdoc),
            "v2" => spec::status_seal(&p, &pid, &patched(sdoc.clone(), &[("v", Value::from(2))])),
            "lean" => spec::status_seal(&p, &pid, &lean),
            "want" => obj! {
                "machine" => "box", "more" => 3, "at" => now - 30.0,
                "cost" => obj! { "week_end" => week_end, "day_end" => day_end, "cli_since" => now - 7200.5 },
                "names" => list!["alice", "bob", "carol", "dave", "gem", "fay", "gus", "hal", "ivy"],
                "exhausted" => obj! {
                    id('a') => list![list!["five_hour", now + 3600.0]], id('d') => list![list!["five_hour", now + 600.0]],
                    id('e') => list![list!["seven_day", now + 4000.0]]
                },
                "near" => list!["hal"], "groups" => list!["Gemini 模型", "Claude、GPT 模型"],
                "first" => obj! { "aid" => "ab".repeat(16), "color" => "#7C3AED", "badge" => "AL", "plan" => "Max", "tier" => "premium", "five_resets" => now + 3600.0 }
            }
        },
    );
    // 额度的通知（辅助程序 status.rs 的 usage_note 封的那种：kind 是 usage）：字在手机上照窗口、重置时间写，时间是手机的时区（这里用 UTC）
    let (atag, uaid) = (crypto::token_hex(8), crypto::token_hex(16));
    let unote = |patch: &[(&str, Value)]| {
        let doc = obj! {
            "v" => 1, "id" => crypto::token_hex(8), "kind" => "usage", "event" => "used_up", "at" => now, "account" => &atag,
            "tool" => "claude", "name" => "alice", "machine" => "box", "window" => "five_hour", "resets_epoch" => now + 3600.0,
            "title" => "额度用完了", "subtitle" => "alice · Claude Code", "body" => "宿主写的", "aid" => &uaid
        };
        patched(doc, patch)
    };
    let who = "来自「Mac mini」 · alice · Claude Code";
    v.insert(
        "usage",
        obj! {
            "p" => hx(&p), "pid" => &pid, "now" => now, "account" => &atag, "aid" => &uaid, "resets" => now + 3600.0,
            "used" => spec::note_seal(&p, &pid, &unote(&[])),
            "used_tomorrow" => spec::note_seal(&p, &pid, &unote(&[("window", Value::from("seven_day")), ("resets_epoch", Value::from(now + 4.0 * 3600.0))])),
            "back" => spec::note_seal(&p, &pid, &unote(&[("event", Value::from("back")), ("at", Value::from(now + 3600.0))])),
            "bad_window" => spec::note_seal(&p, &pid, &unote(&[("window", Value::from("fortnight"))])),
            "other_kind" => spec::note_seal(&p, &pid, &unote(&[("kind", Value::from("weather"))])),
            "bad_account" => spec::note_seal(&p, &pid, &unote(&[("account", Value::from("ABC"))])),
            "no_event" => spec::note_seal(&p, &pid, &unote(&[("event", Value::Null)])),
            "want" => obj! {
                "used" => list!["额度用完了", who, "5 小时的额度用完了，22:20 恢复"],
                "used_tomorrow" => list!["额度用完了", who, "本周的额度用完了，明天 01:20 恢复"],
                "back" => list!["额度恢复了", who, "5 小时的额度恢复了，可以接着用了"]
            }
        },
    );
    // 握手和记录：这边是宿主
    let csid = crypto::token_hex(16);
    let mut viewer = Channel::new("viewer", &p, &pid, &csid, &fpa, &fpb);
    let mut host = Channel::new("host", &p, &pid, &csid, &fpa, &fpb);
    let nv = crypto::random(32);
    let hello = viewer.hello_with(&nv);
    let reply = host.reply(&hello).expect("宿主认 hello");
    let confirm = viewer.confirm(&reply).expect("接入方认宿主的证明");
    assert!(host.accept(&confirm));
    let lens = [0usize, 1, 30, 31, 1000, 16384];
    let records: Vec<Value> = lens.iter().map(|n| Value::from(hx(&host.seal(&pattern(*n))))).collect();
    let tail = hx(&host.seal(b"\x09"));
    v.insert(
        "chan",
        obj! {
            "p" => hx(&p), "pid" => &pid, "sid" => &csid, "fph" => &fpa, "fpv" => &fpb, "nv" => hx(&nv), "hello" => hx(&hello),
            "reply" => hx(&reply), "confirm" => hx(&confirm), "records" => Value::List(records),
            "lens" => Value::List(lens.iter().map(|n| Value::from(*n)).collect()), "tail" => tail
        },
    );
    // 开场白
    let (tid, tkey, cn) = (crypto::token_hex(16), crypto::random(32), crypto::token_hex(8));
    let (line, ok) = spec::relay_line_with("view", &tid, &tkey, &cn);
    let utf = |b: Vec<u8>| String::from_utf8(b).expect("开场白是 ASCII");
    v.insert("relay", obj! { "tid" => &tid, "tkey" => hx(&tkey), "cn" => &cn, "line" => utf(line), "expect" => utf(ok) });
    // 证书：EC（cca 的服务器都是这种）、RSA、Ed25519
    pki::make_cert(&d);
    let mut spki = vec![
        obj! { "what" => "EC P-256", "der" => hx(&der_of(&d.join("cert.pem"))), "pin" => pki::openssl_pin(d.join("cert.pem")) },
    ];
    for (what, newkey) in [("RSA", "rsa:2048"), ("Ed25519", "ed25519")] {
        let c = d.join(format!("{what}.pem"));
        let mut cmd = Cmd::new("openssl");
        cmd.args(["req", "-x509", "-newkey", newkey, "-nodes", "-keyout"]).arg(d.join("k.pem"));
        cmd.args(["-subj", "/CN=x", "-days", "2", "-out"]).arg(&c).pass_env(&["PATH"]).stdout(Stdio::Null).stderr(Stdio::Null);
        cmd.output().unwrap_or_else(|e| panic!("{e}")).check();
        spki.push(obj! { "what" => what, "der" => hx(&der_of(&c)), "pin" => pki::openssl_pin(&c) });
    }
    v.insert("spki", Value::List(spki));
    // 二维码里的链接（cli/src/cmd/remote/pair.rs 的 app_link 出的那种）
    let pin = pki::openssl_pin(d.join("cert.pem"));
    let ctext = spec::code_text(code);
    let bare = ctext.replace('-', "");
    let url = "https://192.0.2.10:18443";
    let base = format!("cca-remote://pair?v=1&u={}&p={}&c={bare}&n={}", enc(url), enc(&pin), enc("我的 Mac"));
    let link = |link: String, pins: Value, team: Value| {
        obj! { "link" => link, "url" => url, "pins" => pins, "code" => &ctext, "name" => "我的 Mac", "team" => team }
    };
    v.insert(
        "links",
        list![
            link(base.clone(), list![&pin], Value::Null),
            link(format!("{base}&t={}", enc("Team A")), list![&pin], Value::from("Team A")),
            link(format!(" {base}&p={}\n", enc(&pin)), list![&pin, &pin], Value::Null),
            link(base.replace(&format!("&p={}", enc(&pin)), ""), list![], Value::Null)
        ],
    );
    let many_pins: String = (0..4).map(|_| format!("&p={}", enc(&pin))).collect();
    v.insert(
        "bad_links",
        list![
            base.replace("v=1", "v=2"),
            base.replace("cca-remote://pair", "cca-remote://join"),
            base.replace("cca-remote", "https"),
            base.replace(&enc("https://"), &enc("http://")),
            base.replace(&bare, &"U".repeat(16)),
            format!("{base}&c={ctext}"),
            base.replace(&enc(&pin), &enc("sha256//abc=")),
            format!("{base}{many_pins}"),
            base.replace(&enc(url), &enc("https://u@192.0.2.10/")),
            "https://example.com/",
            ""
        ],
    );
    // 启动器的说明（远程启动，docs/dev/launch.md：辅助程序 template.rs 的 launch_doc、crypto.rs 的 launch_seal 封的那种）：解开、清理，样子不对的
    // 模板（编号、摘要不是小写十六进制，不认识的工具，重复的编号）丢掉，能挑的账号（3.7.10 起的 accounts）也一样；旧版的 cca 的说明没有
    // accounts。会话的说明的密钥解不开它，它的密钥也解不开会话的说明。LAUNCH 由这一份拼（下面核对），LAUNCHED 照样读
    let (lp, lpid, lsid) = (crypto::random(32), crypto::token_hex(16), crypto::token_hex(16));
    let tpl = |i: &str, h: &str, n: &str, k: &str| {
        obj! { "i" => i, "h" => h, "n" => n, "k" => k, "a" => "a", "d" => "~", "x" => "", "l" => false }
    };
    let ldoc = obj! {
        "v" => 1, "machine" => "box\u{202e}\u{1b}", "more" => 3,
        "t" => list![
            obj! {
                "i" => "0123abcd", "h" => "0123456789abcdef", "n" => "工作\u{1b}[31m", "k" => "claude", "a" => "alice",
                "aid" => "ab".repeat(16), "d" => "~/work", "x" => "--model opus", "l" => true
            },
            obj! {
                "i" => "89abcdef", "h" => "fedcba9876543210", "n" => "x".repeat(60), "k" => "codex", "a" => "bob", "aid" => "XYZ",
                "d" => format!("~/{}", "y".repeat(250)), "x" => "z".repeat(400), "l" => false
            },
            tpl("XYZ12345", "0123456789abcdef", "编号不对", "claude"),
            tpl("00000000", "0123", "摘要不对", "claude"),
            tpl("11111111", "1111111111111111", "别的工具", "vim"),
            tpl("0123abcd", "1111111111111111", "重复的编号", "claude")
        ],
        "accounts" => list![
            obj! { "i" => "0011223344556677", "k" => "claude", "n" => "主力\u{1b}[31m", "aid" => "cd".repeat(16) },
            obj! { "i" => "8899aabbccddeeff", "k" => "codex", "n" => "x".repeat(80), "aid" => "XYZ" },
            obj! { "i" => "0011223344556677", "k" => "claude", "n" => "重复的编号" },
            obj! { "i" => "00112233", "k" => "claude", "n" => "编号不对" },
            obj! { "i" => "FFFFFFFFFFFFFFFF", "k" => "claude", "n" => "大写的编号" },
            obj! { "i" => "1111222233334444", "k" => "vim", "n" => "别的工具" },
            obj! { "k" => "claude", "n" => "没有编号" }
        ]
    };
    let launched = |payload: &str, want: Value| obj! { "payload_hex" => hx(payload.as_bytes()), "want" => want };
    let new_sid = crypto::token_hex(16);
    let ok_reply = |sid: &str, v: i64| format!("{{\"v\":{v},\"ok\":true,\"sid\":\"{sid}\"}}");
    v.insert(
        "launch",
        obj! {
            "p" => hx(&lp), "pid" => &lpid, "sid" => &lsid, "other_sid" => crypto::token_hex(16),
            "blob" => spec::launch_seal(&lp, &lpid, &lsid, &ldoc),
            "forged" => spec::launch_seal(&crypto::random(32), &lpid, &lsid, &ldoc),
            "dir" => spec::dir_seal(&lp, &lpid, &lsid, &obj! { "v" => 1, "machine" => "m", "tool" => "claude", "name" => "n", "cwd" => "~", "since" => 1 }),
            "old_blob" => spec::launch_seal(&lp, &lpid, &lsid, &obj! { "v" => 1, "machine" => "m", "more" => 0, "t" => list![tpl("0a0b0c0d", "0123456789abcdef", "旧的", "claude")] }),
            "want" => obj! {
                "machine" => "box", "more" => 3,
                "t" => list![
                    obj! {
                        "i" => "0123abcd", "h" => "0123456789abcdef", "n" => "工作[31m", "k" => "claude", "a" => "alice",
                        "aid" => "ab".repeat(16), "d" => "~/work", "x" => "--model opus", "l" => true
                    },
                    obj! {
                        "i" => "89abcdef", "h" => "fedcba9876543210", "n" => "x".repeat(40), "k" => "codex", "a" => "bob", "aid" => "",
                        "d" => format!("~/{}", "y".repeat(198)), "x" => "z".repeat(300), "l" => false
                    }
                ],
                "accounts" => list![
                    obj! { "i" => "0011223344556677", "k" => "claude", "n" => "主力[31m", "aid" => "cd".repeat(16) },
                    obj! { "i" => "8899aabbccddeeff", "k" => "codex", "n" => "x".repeat(64), "aid" => "" }
                ]
            },
            "ask" => obj! { "t" => "0123abcd", "h" => "0123456789abcdef", "c" => 100, "r" => 30, "l" => "你好", "a" => "0011223344556677" },
            "bad_lines" => list!["", "   ", "a\u{1b}b", "a\nb", "字".repeat(501)],
            "launched" => list![
                launched(&ok_reply(&new_sid, 1), obj! { "ok" => true, "sid" => &new_sid }),
                launched(
                    "{\"v\":1,\"ok\":false,\"code\":\"changed\",\"text\":\"模板\\n改过了\"}",
                    obj! { "ok" => false, "code" => "changed", "text" => "模板改过了" }
                ),
                launched(&ok_reply(&new_sid, 2), Value::Null),
                launched(&ok_reply("xyz", 1), Value::Null),
                launched(&ok_reply(&new_sid.to_uppercase(), 1), Value::Null),
                launched("{\"v\":1,\"ok\":false,\"code\":\"Bad Code\",\"text\":\"x\"}", Value::Null),
                launched("nope", Value::Null)
            ]
        },
    );
    let data = b"{\"op\":\"relay_list\",\"pids\":[]}";
    v.insert("sign", obj! { "raw" => crypto::token_hex(32), "data" => hx(data) });
    fsx::write(d.join("in.json"), json::dumps(&Value::Obj(v), &Style::default()));

    let mut cmd = Cmd::new(&exe);
    cmd.arg(d.join("in.json")).arg(d.join("out.json")).timeout(secs(120));
    let r = cmd.output().unwrap_or_else(|e| panic!("{e}"));
    let said = format!("{}{}", r.out(), r.err());
    assert!(r.status.success(), "{said}");
    assert!(relay::matches(r"通过 \d+，失败 0", &r.out()), "{said}");
    let out = fsx::read_json(d.join("out.json"));
    // 这一份封的配对消息：kit::spec 解得开、内容对
    let theirs = spec::pair_open(&keys, &rid, "b", &out["msg_b"]).expect("Swift 封的配对消息认证得过");
    assert_eq!(relay::cols(&theirs, &["v", "fp", "n", "name"]), list![1, &fpb, hx(&nb), "我的 iPhone"]);
    let c = crypto::b64_decode(out["msg_b"]["c"].str()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.len() % 256, 0, "补齐到 256 的整数倍");
    // 这一份发的记录：宿主收得到
    let mut buf: Vec<u8> = out["records"].list().iter().flat_map(|x| crypto::unhex(x.str()).expect("十六进制")).collect();
    let got: Vec<Vec<u8>> = lens.iter().map(|_| host.open(&mut buf).expect("记录认证得过").expect("够一条")).collect();
    assert_eq!(got, lens.iter().map(|n| pattern(*n)).collect::<Vec<_>>());
    assert!(buf.is_empty(), "多出来 {} 个字节", buf.len());
    // 这一份拼的 LAUNCH：类型、长度、内容（JSON；开场白、账号只在有的时候带）
    let launch_frame = |key: &str| {
        let raw = crypto::unhex(out[key].str()).unwrap_or_else(|e| panic!("{key} 不是十六进制：{e}"));
        assert_eq!(raw.first(), Some(&relay::LAUNCH), "{key}");
        let n = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize;
        assert_eq!(raw.len(), 5 + n, "{key}");
        relay::parsed(std::str::from_utf8(&raw[5..]).expect("LAUNCH 是 UTF-8"))
    };
    let got = launch_frame("launch_frame");
    assert_eq!(
        (got.obj().len(), relay::cols(&got, &["v", "t", "h", "c", "r", "l", "a"])),
        (7, list![1, "0123abcd", "0123456789abcdef", 100, 30, "你好", "0011223344556677"])
    );
    let got = launch_frame("launch_frame_plain");
    assert_eq!(
        (got.obj().len(), relay::cols(&got, &["v", "t", "h", "c", "r"])),
        (5, list![1, "0123abcd", "0123456789abcdef", 100, 30])
    );
    // 签名：ssh-keygen 认
    let s = &out["sign"];
    assert!(s["pub"].str().starts_with("ssh-ed25519 "), "{s}");
    fsx::write(d.join("pub"), format!("{}\n", s["pub"].str()));
    assert_eq!(s["fp"].str(), spec::fp_of(d.join("pub")));
    assert!(spec::ssh_verify(s["pub"].str(), "cca-presence", data, s["sig"].str().as_bytes()), "ssh-keygen 不认 Swift 的签名：{s}");
}

// ---- 手机那一头：ios/tools/ccaguest ----

/// 手机那一头：ios/tools/ccaguest，配对表、密钥都在自己的目录里
struct Guest {
    exe: PathBuf,
    dir: PathBuf,
    name: String,
}

/// ccaguest 跑完的结果
struct Done {
    code: i32,
    out: String,
    err: String,
}

impl Guest {
    fn new(fx: &Base, name: &str) -> Guest {
        let exe = build("ccaguest", &harness::repo().join("ios").join("tools").join("ccaguest"));
        let dir = fx.d.join(format!("guest-{}", crypto::token_hex(3)));
        std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        Guest { exe, dir, name: name.to_string() }
    }

    fn cmd(&self, args: &[&str]) -> Cmd {
        let mut cmd = Cmd::new(&self.exe);
        cmd.arg("--store").arg(&self.dir).args(["--name", self.name.as_str()]).args(args);
        cmd
    }

    /// 跑一条命令（标准输入是 input），等它跑完（最多 90 秒）
    fn run_with(&self, args: &[&str], input: &str) -> Done {
        let r = self.cmd(args).input(input).timeout(secs(90)).output().unwrap_or_else(|e| panic!("{e}"));
        Done { code: r.status.py(), out: r.out(), err: r.err() }
    }

    fn run(&self, args: &[&str]) -> Done {
        self.run_with(args, "")
    }

    /// 该成的：退出码不是 0 就 panic；给出标准输出
    #[track_caller]
    fn ok(&self, args: &[&str]) -> String {
        let r = self.run(args);
        assert_eq!(r.code, 0, "{} {}", r.out, r.err);
        r.out
    }

    /// 列出来的（一行一个 JSON）
    fn rows(&self) -> Vec<Value> {
        self.ok(&["list"]).lines().map(relay::parsed).collect()
    }

    /// 接入（不等）：敲的键写到它的标准输入，终端的字节、它说的状态（state open……）一直有线在收
    fn attach(&self, pid: &str, sid: &str, flags: &[&str]) -> Attached {
        let mut cmd = self.cmd(&[&["attach", pid, sid], flags].concat());
        cmd.stdin(Stdio::Piped).stdout(Stdio::Piped).stderr(Stdio::Piped);
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{e}"));
        let (out, err) = (collect(child.stdout.take().expect("管道")), collect(child.stderr.take().expect("管道")));
        let stdin = child.stdin.take();
        Attached { child, stdin, out, err }
    }
}

/// 另起一条线一直读，读到的攒起来
fn collect(mut from: File) -> Arc<Mutex<Vec<u8>>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
        }
    });
    got
}

/// 接着会话的 ccaguest
struct Attached {
    child: Child,
    stdin: Option<File>,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
}

impl Attached {
    fn out(&self) -> Vec<u8> {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn err(&self) -> Vec<u8> {
        self.err.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 敲键
    fn type_in(&mut self, keys: &[u8]) {
        let stdin = self.stdin.as_mut().expect("标准输入还开着");
        stdin.write_all(keys).and_then(|_| stdin.flush()).unwrap_or_else(|e| panic!("{e}"));
    }

    /// 等它说 state open
    fn wait_open(&self) {
        wait_for("接上了", secs(30), || has(&self.err(), "state open"));
    }

    /// 关上标准输入（读到头：断开），等它退出：退出码
    fn finish(&mut self) -> i32 {
        drop(self.stdin.take());
        self.wait()
    }

    fn wait(&mut self) -> i32 {
        match self.child.wait_timeout(secs(30)).unwrap_or_else(|e| panic!("{e}")) {
            Some(exit) => exit.py(),
            None => panic!("ccaguest 30 秒还没退出：{}", String::from_utf8_lossy(&self.err())),
        }
    }
}

/// 开着中转的服务器，box 登记在上面
fn guest_fx(t: &mut Ctx) -> (Base, Device) {
    let fx = Base::new(t);
    let a = fx.device("box");
    fx.machines(&[&a]);
    (fx, a)
}

/// box 上 remote-pair-show --web（不等它配完）；给出进程和 link 那一行里的链接
fn show(a: &Device) -> (Popen, String) {
    let mut p = Popen::spawn(&mut relay::helper_cmd(a, &["remote-pair-show", "--web"]));
    let first = p.out_line();
    assert!(first.starts_with("code "), "{first:?}");
    let line = p.out_line();
    let link = line.strip_prefix("link cca-remote://").map(|_| line[5..].trim().to_string());
    (p, link.unwrap_or_else(|| panic!("配对码后面该是给 App 的链接：{line:?}")))
}

/// box 上 remote-pair-show --web，手机拿 link 那一行配对。回 (手机说的那一行, box 说的对方的名字)
fn pair(a: &Device, g: &Guest) -> (String, String) {
    let (mut p, link) = show(a);
    let out = g.ok(&["pair", &link]);
    let (rc, said, err) = p.communicate(secs(60));
    assert_eq!((rc, err.as_str()), (0, ""));
    (out, said.trim().trim_start_matches("peer ").to_string())
}

/// 服务器记着的配对和访客（d/remote.json）
fn remote(d: &Path) -> Value {
    fsx::read_json(d.join("remote.json"))
}

/// 手机列出来的会话（有 sid 的那几行）：等到正好一个
fn one_session(g: &Guest) -> Value {
    wait_for("手机看到会话", secs(30), || {
        let rows: Vec<Value> = g.rows().into_iter().filter(|r| r.get("sid").is_some()).collect();
        (rows.len() == 1).then(|| rows[0].clone())
    })
}

fn pair_list_attach_type(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    assert_eq!(g.ok(&["list"]), "", "没配对过：什么都没有");
    let (out, said) = pair(&a, &g);
    assert_eq!(said, "我的 iPhone");
    let words: Vec<&str> = out.splitn(3, ' ').collect();
    assert_eq!((words.len(), words[0], words.get(2).map(|n| n.trim())), (3, "peer", Some("box")), "{out:?}");
    let pid = words[1].to_string();
    let row = relay::peers(&a)[pid.as_str()].clone();
    assert_eq!(relay::cols(&row, &["incoming", "web"]), list!["control", true]);
    let peers = a.ok(&["remote-peers"]);
    assert!(peers.contains("我的 iPhone（手机）"), "{peers}");
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    let r = one_session(&g);
    assert_eq!(relay::cols(&r, &["pid", "tool", "online", "view"]), list![&pid, "claude", true, false], "{r}");
    assert!(!r["machine"].str().is_empty(), "{r}");
    let sid = r["sid"].str().to_string();
    let mark = h.mark();
    let mut p = g.attach(&pid, &sid, &["--size", "40x20"]);
    p.wait_open();
    // App 说自己能缩放：一接上会话就照它的窗口排，宿主告诉它会话多大（ccaguest 说的 size 是列、行，最后说的那一次是现在的）
    let last_size = |p: &Attached| -> Option<String> {
        let err = String::from_utf8_lossy(&p.err()).into_owned();
        err.lines().filter_map(|l| l.strip_prefix("size ")).last().map(str::to_string)
    };
    h.expect_since("<size 20 40>", mark);
    wait_for("手机知道会话照它排了", secs(20), || (last_size(&p).as_deref() == Some("40 20")).then_some(()));
    // 这边打了字：又照这边的终端排（手机缩小了画）；手机上打了字再照它排
    let mark = h.mark();
    h.send(b"say sync\r");
    h.expect_since("<sync>", mark);
    h.expect_since("<size 24 80>", mark);
    wait_for("手机知道会话照这边排了", secs(20), || (last_size(&p).as_deref() == Some("80 24")).then_some(()));
    let mark = h.mark();
    p.type_in(b"say hello-from-the-phone\r");
    h.expect("<hello-from-the-phone>"); // 会话程序收到了（宿主那边的终端里看得到它的回答）
    h.expect_since("<size 20 40>", mark);
    wait_for("手机收到会话的输出", secs(20), || has(&p.out(), "<hello-from-the-phone>"));
    wait_for("手机知道会话又照它排了", secs(20), || (last_size(&p).as_deref() == Some("40 20")).then_some(()));
    // 会话里的钩子推一条通知（cca remote notify）：手机收到 CcaRemoteKit 解好的
    h.send(format!("run {} remote-notify --title 做完了 --subtitle alice 第一行回复\n", relay::helper(&a).display()));
    // 新版的宿主给每一条一个编号（经推送也来了一份时照它只弹一次）
    let note = r#"note \{"body":"第一行回复","id":"[0-9a-f]{16}","subtitle":"alice","title":"做完了"\}"#;
    wait_for("手机收到通知", secs(20), || relay::matches(note, &String::from_utf8_lossy(&p.err())));
    assert_eq!(p.finish(), 0, "{}", String::from_utf8_lossy(&p.err())); // 读到头：断开（会话接着运行）
    assert!(has(&p.err(), "closed detached"), "{}", String::from_utf8_lossy(&p.err()));
    h.send(b"say still-here\r");
    h.expect("<still-here>");
    // 只看：敲的不送
    let mut q = g.attach(&pid, &sid, &["--view"]);
    q.wait_open();
    q.type_in(b"say not-sent\r");
    h.send(b"say from-the-mac\r");
    wait_for("只看的也收到输出", secs(20), || has(&q.out(), "<from-the-mac>"));
    assert_eq!(q.finish(), 0);
    assert!(!has(&q.out(), "<not-sent>"), "只看：敲的不送");
    // 取消配对：服务器上的记录没了
    assert_eq!(g.ok(&["unpair", &pid]).trim(), "unpaired");
    assert_eq!(g.ok(&["list"]), "");
    let doc = remote(&fx.d);
    assert_eq!((&doc["edges"], &doc["guests"]), (&obj! {}, &obj! {}));
}

fn wrong_pin_and_used_code(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    let (mut p, link) = show(&a);
    // 证书和二维码里的指纹对不上（服务器被换掉了、路上有人）：一个请求都不发成
    let other = format!("sha256//{}", crypto::b64(&crypto::random(32)));
    let bad = link.replace(&enc(&fx.pin), &enc(&other));
    assert_ne!(bad, link);
    let r = g.run(&["pair", &bad]);
    assert_eq!((r.code, r.out.trim()), (1, "fail pin"), "{}", r.err);
    assert_eq!(g.run(&["pair", "cca-remote://pair?v=1"]).out.trim(), "fail link");
    g.ok(&["pair", &link]);
    assert_eq!(p.wait(secs(60)), 0);
    // 同一个配对码只能用一次
    let r = Guest::new(&fx, "iPad").run(&["pair", &link]);
    assert_eq!((r.code, r.out.trim()), (1, "fail no_pairing"), "{}", r.err);
}

fn session_ends(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    let (out, _) = pair(&a, &g);
    let pid = out.split_whitespace().nth(1).expect("peer 后面是配对编号").to_string();
    let _h = relay::session(&a, Device::KEY, "alice", &[]);
    let r = one_session(&g);
    assert_eq!(r["end"], Value::Bool(false), "终端里起的会话：手机结束不了（只在那台电脑上 cca remote end）");
    let sid = r["sid"].str().to_string();
    // 硬要结束（END）：宿主那头的转接进程不替手机转，会话照常——同一条连接上接着敲的键还到得了
    let mut p = g.attach(&pid, &sid, &["--end"]);
    p.wait_open();
    wait_for("请宿主结束", secs(10), || has(&p.err(), "end sent"));
    p.type_in(b"say still-here\r");
    wait_for("会话还在", secs(20), || has(&p.out(), "<still-here>"));
    p.type_in(b"exit 3\r"); // 会话程序退出（退出码 3）
    let rc = p.wait();
    assert!(has(&p.err(), "exit 3\nclosed ended"), "{}", String::from_utf8_lossy(&p.err()));
    assert_eq!(rc, 0);
    // 会话没了：再接是 gone
    let r2 = g.run_with(&["attach", &pid, &sid], "");
    assert!(r2.err.contains("closed gone"), "{}", r2.err);
}

/// 远程启动（docs/dev/launch.md）：手机（ccaguest，和 App 同一份 CcaRemoteKit）列出来那台机器的启动器和模板，照模板起一个会话，
/// 马上接进去（刚起的会话服务器可能还不知道：--fresh 接着问）。启动器起的会话里的“cca”是假客户端（CCA_SELF 指着它）
fn a_phone_launches_a_session(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    let (out, _) = pair(&a, &g);
    let pid = out.split_whitespace().nth(1).expect("peer 后面是配对编号").to_string();
    let work = relay::work(&a);
    let dir = work.to_str().expect("临时目录的路径是 UTF-8");
    let added = a.ok(&["remote-template", "add", "修 bug", "claude", dir, "--", "--model", "opus"]);
    let tid = added.split('\t').next().expect("编号").to_string();
    a.ok(&["remote-allow", &pid, "launch", "on"]);
    let mut cmd = relay::helper_cmd(&a, &["remote-launcher", "serve"]);
    cmd.env("CCA_SELF", relay::client(&a));
    let mut l = Popen::spawn(&mut cmd);
    // 手机上：那台机器有启动器（不算会话），模板的编号、名字、工具、账号（能挑的只有 alice：给旧版的 App 看的就是它）、文件夹、参数、
    // 能带开场白；能挑的账号（编号按配对算的）
    let row = wait_for("手机看到启动器", secs(30), || g.rows().into_iter().find(|r| r["launcher"].as_obj().is_some()));
    let launcher = &row["launcher"];
    assert_eq!(launcher["online"], Value::Bool(true), "{row}");
    let first = &launcher["t"].list()[0];
    assert_eq!(
        relay::cols(first, &["i", "n", "k", "a", "d", "x", "l"]),
        list![&tid, "修 bug", "claude", "alice", "~/work", "--model opus", true]
    );
    let accts = launcher["accounts"].list();
    assert_eq!(accts.len(), 1, "{launcher}");
    assert_eq!(relay::cols(&accts[0], &["k", "n", "aid"]), list!["claude", "alice", first["aid"].str()]);
    let acct = accts[0]["i"].str().to_string();
    assert!(acct.len() == 16 && acct.bytes().all(|c| c.is_ascii_hexdigit()), "{acct}");
    assert!(g.rows().iter().all(|r| r.get("sid").is_none()), "启动器不是会话");
    // 起一个：挑 alice、带开场白，窗口是手机说的；起来了马上接进去
    let r = g.run(&["launch", &pid, &tid, "--size", "120x40", "--line", "看一下 README", "--account", &acct]);
    assert_eq!(r.code, 0, "{} {}", r.out, r.err);
    let sid = r.out.trim().strip_prefix("launched ").unwrap_or_else(|| panic!("{:?}", r.out)).to_string();
    let mut p = g.attach(&pid, &sid, &["--fresh", "--size", "120x40"]);
    p.wait_open();
    wait_for("手机看到会话里的客户端", secs(20), || has(&p.out(), "'--model', 'opus', '--', '看一下 README']>"));
    p.type_in(b"exit 0\r");
    assert_eq!(p.wait(), 0, "{}", String::from_utf8_lossy(&p.err()));
    assert!(has(&p.err(), "closed ended"), "{}", String::from_utf8_lossy(&p.err()));
    // 再起一个，在手机上结束它：说明里说这台手机结束得了（远程启动的、能操作的配对）；接上就请宿主结束（END），
    // 里面的程序收到 SIGHUP（128 + 1），手机收到退出码，会话没了
    std::thread::sleep(Duration::from_millis(3200)); // 同一台手机两次接进启动器要隔 3 秒（launch.rs 的 JOIN_GAP）
    let r = g.run(&["launch", &pid, &tid, "--account", &acct]);
    assert_eq!(r.code, 0, "{} {}", r.out, r.err);
    let sid2 = r.out.trim().strip_prefix("launched ").unwrap_or_else(|| panic!("{:?}", r.out)).to_string();
    let row2 = wait_for("手机看到新会话", secs(30), || {
        g.rows().into_iter().find(|r| r.get("sid").is_some_and(|s| s.str() == sid2))
    });
    assert_eq!(row2["end"], Value::Bool(true), "{row2}");
    let mut p = g.attach(&pid, &sid2, &["--end"]);
    assert_eq!(p.wait(), 0, "{}", String::from_utf8_lossy(&p.err()));
    let said = String::from_utf8_lossy(&p.err()).to_string();
    assert!(said.contains("end sent\n") && said.contains("exit 129\nclosed ended"), "{said}");
    let again = g.run_with(&["attach", &pid, &sid2], "");
    assert!(again.err.contains("closed gone"), "{}", again.err);
    // 关掉远程启动：启动器自己退出，手机上看不到它了
    a.ok(&["remote-allow", &pid, "launch", "off"]);
    assert_eq!(l.wait(secs(20)), 0);
    wait_for("手机上启动器没了", secs(30), || g.rows().iter().all(|r| r["launcher"].is_null()));
}

/// 推送：手机（ccaguest，和 App、通知扩展同一份 CcaRemoteKit）在服务器上登记推送令牌；box、mini 两台都和它配了对，mini 上的会话
/// 说一条通知，经服务器、推送网关（假的，kit::apnsfake）到推送服务（假的）；推送服务收到的那一份，手机拿每个配对的通知密钥试着解：
/// 解开的是 mini 的、说话的是那个会话。登记里说了只推哪几个会话（「已关闭的会话也推送」关着）：别的会话的不推，说了的照推。
/// 取消和 mini 的配对以后解不开了（通知扩展照推送里写的那句「有新的通知」说）
fn pushes_reach_the_phone(t: &mut Ctx) {
    let gw = Gateway::start(&t.tmp("cca-push-"), Knobs::default());
    let push = vec!["--push".to_string(), gw.url(), "--push-pin".to_string(), gw.pin.clone()];
    let fx = Base::with(t, true, move |_| push);
    let (a, b) = (fx.device("box"), fx.device("mini"));
    fx.machines(&[&a, &b]);
    let g = Guest::new(&fx, "我的 iPhone");
    // 推送服务都回 200（最后两个字符定它回什么：kit::apnsfake 的 by_tail）
    let token = Gateway::token("aa");
    // 还没配对：没有服务器可登记
    assert_eq!(g.ok(&["push-set", &token, "sandbox"]), "");
    let pid_of = |out: &str| out.split_whitespace().nth(1).expect("peer 后面是配对编号").to_string();
    let pid_a = pid_of(&pair(&a, &g).0);
    let pid_b = pid_of(&pair(&b, &g).0);
    // 两台机器在同一台服务器上：登记一次
    let said = g.ok(&["push-set", &token, "sandbox"]);
    assert_eq!(said.trim(), format!("push {} ok", fx.url()), "{said}");
    let guests = remote(&fx.d)["guests"].clone();
    let regs: Vec<Value> = guests.obj().values().map(|g| relay::cols(&g["push"], &["token", "env"])).collect();
    assert_eq!(regs, [list![&token, "sandbox"]], "{guests}");
    let mut h = relay::session(&b, Device::KEY, "alice", &[]);
    let rows = wait_for("手机看到 mini 上的会话", secs(30), || {
        let rows: Vec<Value> = g.rows().into_iter().filter(|r| r.get("sid").is_some() && r["pid"].str() == pid_b).collect();
        (!rows.is_empty()).then_some(rows)
    });
    h.send(format!("run {} remote-notify --title 做完了 --subtitle alice 第一行回复\n", relay::helper(&b).display()));
    let seen = gw.sandbox.wait_seen(1, secs(30)).remove(0);
    assert_eq!(seen.token(), token);
    let file = fx.d.join("pushed.json");
    fsx::write(&file, &seen.body);
    let file = text(&file);
    let n = relay::parsed(&g.ok(&["push-open", &file]));
    assert_eq!(
        relay::cols(&n, &["pid", "machine", "title", "subtitle", "body"]),
        list![&pid_b, "mini", "做完了", "alice", "第一行回复"]
    );
    assert_eq!(n["card"], list!["做完了", "来自「mini」 · alice", "第一行回复"]);
    assert_eq!(n["sound"].str(), "done", "标题以「做完了」结尾：响做完了的那一声");
    assert_eq!(n["id"].str(), seen.header("apns-collapse-id"), "推送服务照通知的编号合并");
    assert_eq!(n["sid"], rows[0]["sid"], "点开通知接入的就是说话的这个会话");
    assert!(!String::from_utf8_lossy(&seen.body).contains("第一行回复"), "推送服务看得到通知里的字");
    // 只推 App 里开着的会话（push_set 带 sids）：另一台手机（g2）说开着的是别的会话，这一条不推给它；g（没带 sids）照推。g 那一份
    // 到了时推不推给 g2 已经定了（宿主一下子交给服务器的，服务器一条线程照先后转），再等一会儿还是只多了 g 那一份
    let sid_b = rows[0]["sid"].str().to_string();
    let g2 = Guest::new(&fx, "另一台 iPhone");
    let pid_b2 = pid_of(&pair(&b, &g2).0);
    // 转接进程每 3 秒看一眼配对表：等 g2 看得到这个会话（它也就在推给谁的里面了）。隔得开一点问：服务器给每台设备限了流
    let end = Instant::now() + secs(30);
    while !g2.rows().iter().any(|r| r.get("sid").is_some() && r["pid"].str() == pid_b2) {
        assert!(Instant::now() < end, "另一台手机一直看不到 mini 上的会话");
        std::thread::sleep(Duration::from_millis(500));
    }
    let token2 = Gateway::token("aa");
    let only = |sids: &str| g2.ok(&["push-set", &token2, "sandbox", "--only", sids]).trim().to_string();
    let elsewhere = "0".repeat(32);
    assert_eq!(only(&elsewhere), format!("push {} ok only", fx.url()));
    let guests = remote(&fx.d)["guests"].clone();
    let sids: Vec<Option<&Value>> = guests.obj().values().map(|g| g.get("push").and_then(|p| p.get("sids"))).collect();
    assert!(sids.contains(&Some(&list![&elsewhere])), "{guests}");
    h.send(format!("run {} remote-notify --title 第二条 --subtitle alice 只推开着的\n", relay::helper(&b).display()));
    assert_eq!(gw.sandbox.wait_seen(2, secs(30))[1].token(), token);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(gw.sandbox.seen().len(), 2, "开着的不是这个会话：不该推给 g2");
    // 开着的就是这个会话：推
    assert_eq!(only(&sid_b), format!("push {} ok only", fx.url()));
    h.send(format!("run {} remote-notify --title 第三条 --subtitle alice 开着的会话\n", relay::helper(&b).display()));
    let mut tokens: Vec<String> = gw.sandbox.wait_seen(4, secs(30))[2..].iter().map(|s| s.token().to_string()).collect();
    tokens.sort();
    let mut both = vec![token.clone(), token2.clone()];
    both.sort();
    assert_eq!(tokens, both);
    // 取消和 mini 的配对：它推来的这一份手机就解不开了（box 的那把通知密钥解不开 mini 封的）
    assert_eq!(g.ok(&["unpair", &pid_b]).trim(), "unpaired");
    let r = g.run(&["push-open", &file]);
    assert_eq!((r.code, r.out.trim()), (1, "fail"), "{}", r.err);
    assert!(g.rows().iter().any(|r| r["pid"].str() == pid_a), "box 的配对还在");
    // 拿掉登记（通知被关掉了）：服务器上没有了
    assert_eq!(g.ok(&["push-set", "", "sandbox"]).trim(), format!("push {} ok", fx.url()));
    assert_eq!(g2.ok(&["push-set", "", "sandbox"]).trim(), format!("push {} ok", fx.url()));
    let guests = remote(&fx.d)["guests"].clone();
    assert!(guests.obj().values().all(|g| g.get("push").is_none()), "{guests}");
    h.send("exit 0\n");
    assert_eq!(h.wait(), 0);
    assert_eq!((gw.production.complaints(), gw.sandbox.complaints()), (Vec::<String>::new(), Vec::<String>::new()));
}

/// 手机上的「账号」页和额度的通知（docs/dev/remote.md「手机上的账号」、docs/dev/push.md「额度的通知」）：box 上 alice 的 5 小时
/// 额度用完了、半分钟后重置。App 的协议核心列会话时连这台机器的账号一起取回（名字、用量、用完了没有：`AccountStatus.exhausted`）；
/// 登记推送时带着 usage，用完的那一条 box 马上推、恢复的那一条服务器到点推，像通知扩展那样解开、字在手机上写（`UsageText`）。
/// 登记时不带 usage（手机上关掉了这种通知）就不推，再打开又推
fn accounts_reach_the_phone(t: &mut Ctx) {
    let gw = Gateway::start(&t.tmp("cca-push-"), Knobs::default());
    let push = vec!["--push".to_string(), gw.url(), "--push-pin".to_string(), gw.pin.clone()];
    let fx = Base::with(t, true, move |_| push);
    let a = fx.device("box");
    fx.machines(&[&a]);
    let g = Guest::new(&fx, "我的 iPhone");
    let out = pair(&a, &g).0;
    let pid = out.split_whitespace().nth(1).expect("peer 后面是配对编号").to_string();
    let token = Gateway::token("aa");
    let usage_on = format!("push {} ok usage", fx.url());
    assert_eq!(g.ok(&["push-set", &token, "sandbox", "--usage"]).trim(), usage_on);
    let resets = fleet::now().floor() + 30.0;
    relay::signed_in(&a, 100, resets);
    relay::publish(&a);
    // 列会话时带回来的这台机器的账号（这台机器上没有会话：只有配对那一行）
    let head = wait_for("手机看到 box 上的账号", secs(30), || {
        g.rows().into_iter().find(|r| r["pid"].str() == pid && r.get("sid").is_none() && !r["status"].is_null())
    });
    assert_eq!(head["served"], Value::Bool(true), "{head}");
    let st = &head["status"];
    // 机器名和会话的说明里的一样（这台机器的短主机名）；配对表里叫它 box
    assert!(!st["machine"].str().is_empty() && st["off"] == Value::Bool(false), "{st}");
    assert_eq!(head["peer"].str(), "box", "{head}");
    let acct = relay::only(&st["accounts"]);
    assert_eq!(relay::cols(acct, &["tool", "name", "kind", "login"]), list!["claude", "alice", "oauth", "logged_in"]);
    // 这个配对所在的 Team（team_of）：box 是 Team 里的成员，分给它 alice 一个账号；alice 没在用（没有会话、没开 App）
    let team = &st["team"];
    assert_eq!((team["role"].str(), team["accounts"].f64(), team["coadmin"].clone()), ("member", 1.0, Value::Bool(false)), "{st}");
    assert!(!team["name"].str().is_empty() && !team["host"].str().is_empty() && !team["state"].str().is_empty(), "{team}");
    assert_eq!((acct["app"].clone(), acct["cli"].f64(), acct["since"].is_null()), (Value::Bool(false), 0.0, true), "{acct}");
    assert_eq!((acct["five_hour"]["utilization"].f64(), acct["seven_day"]["utilization"].f64()), (100.0, 41.0));
    let used = &acct["exhausted"];
    assert_eq!((used.list().len(), used[0][0].str(), used[0][1].f64()), (1, "five_hour", resets), "{acct}");
    assert!(!head.compact().contains("example.com"), "不给邮箱：{head}");
    // 用完的那一条：box 马上推；推送服务那里看不到里面的字
    let seen = gw.sandbox.wait_seen(1, secs(30)).remove(0);
    assert_eq!(seen.token(), token);
    assert!(!String::from_utf8_lossy(&seen.body).contains("alice"), "推送服务看得到通知里的字");
    let n = opened(&g, &fx, &seen.body);
    let now = fleet::now();
    assert_eq!((n["pid"].str(), n["id"].str()), (pid.as_str(), seen.header("apns-collapse-id")));
    let u = &n["usage"];
    assert_eq!(
        relay::cols(u, &["event", "window", "tool", "name", "account", "aid"]),
        list!["used_up", "five_hour", "claude", "alice", acct["id"].str(), acct["aid"].str()]
    );
    assert_eq!(u["resets_epoch"].f64(), resets);
    let body = format!("5 小时的额度用完了，{} 恢复", utc_back_time(resets, now));
    assert_eq!(n["card"], list!["额度用完了", "来自「box」 · alice · Claude Code", body]);
    assert_eq!(n["sound"].str(), "ask", "额度用完了：响在等你的那一声");
    // 恢复的那一条：服务器到点推（box 这期间什么都不用做）
    let back = gw.sandbox.wait_seen(2, secs(60)).remove(1);
    assert!(fleet::now() >= resets, "到了重置的时候才推");
    let n2 = opened(&g, &fx, &back.body);
    assert_eq!(relay::cols(&n2["usage"], &["event", "window", "account"]), list!["back", "five_hour", acct["id"].str()]);
    assert_eq!(n2["card"], list!["额度恢复了", "来自「box」 · alice · Claude Code", "5 小时的额度恢复了，可以接着用了"]);
    assert_eq!(n2["sound"].str(), "done", "额度恢复了：响做完了的那一声");
    // 登记时不带 usage：下一轮用完了不推；再带上又推
    assert_eq!(g.ok(&["push-set", &token, "sandbox"]).trim(), format!("push {} ok", fx.url()));
    relay::signed_in(&a, 100, fleet::now().floor() + 4.0 * 3600.0);
    relay::publish(&a);
    std::thread::sleep(secs(2));
    assert_eq!(gw.sandbox.seen().len(), 2, "关掉了额度的通知：不推");
    assert_eq!(g.ok(&["push-set", &token, "sandbox", "--usage"]).trim(), usage_on);
    relay::signed_in(&a, 100, fleet::now().floor() + 8.0 * 3600.0);
    relay::publish(&a);
    let third = gw.sandbox.wait_seen(3, secs(30)).remove(2);
    assert_eq!(opened(&g, &fx, &third.body)["usage"]["event"].str(), "used_up");
    assert_eq!((gw.production.complaints(), gw.sandbox.complaints()), (Vec::<String>::new(), Vec::<String>::new()));
}

/// 像通知扩展那样解开推来的那一份（ccaguest push-open；时区是 UTC，字里的钟点照它写）
fn opened(g: &Guest, fx: &Base, body: &[u8]) -> Value {
    let file = fx.d.join(format!("pushed-{}.json", crypto::token_hex(3)));
    fsx::write(&file, body);
    let path = text(&file);
    let r = g.cmd(&["push-open", &path]).env("TZ", "UTC").timeout(secs(90)).output().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.status.py(), 0, "{} {}", r.out(), r.err());
    relay::parsed(&r.out())
}

/// 手机上说的几点恢复（UTC）：和 now 同一天的“09:05”，第二天的“明天 09:05”（只用在半分钟以内的重置时间上）
fn utc_back_time(epoch: f64, now: f64) -> String {
    let (d, n) = (kit::plist::date_from_unix(epoch as i64), kit::plist::date_from_unix(now as i64));
    let clock = format!("{:02}:{:02}", d.hour, d.minute);
    if (d.year, d.month, d.day) == (n.year, n.month, n.day) {
        clock
    } else {
        format!("明天 {clock}")
    }
}

// ---- 模拟器里真的 App ----

const BUNDLE: &str = "com.cca.remote";

/// ios/test.sh 建的那台模拟器（开着的）：有就回它的 UDID
fn simulator() -> Option<String> {
    let f = harness::repo().join("ios").join("build").join("simulator");
    let udid = fsx::try_read_text(&f)?.trim().to_string();
    let mut cmd = Cmd::new("xcrun");
    cmd.args(["simctl", "list", "devices", "booted", "-j"]).env_inherit().stderr(Stdio::Null);
    let r = cmd.output().ok()?;
    let doc = json::parse(&r.out()).ok()?;
    let booted =
        doc.get("devices")?.obj().values().flat_map(|ds| ds.list().iter().map(|d| d["udid"].str().to_string())).collect::<Vec<_>>();
    booted.contains(&udid).then_some(udid)
}

/// xcrun simctl 的一条命令（不管成不成）
fn simctl(args: &[&str]) -> bool {
    let mut cmd = Cmd::new("xcrun");
    cmd.arg("simctl").args(args).env_inherit().stdout(Stdio::Null).stderr(Stdio::Null).timeout(secs(120));
    cmd.output().is_ok_and(|r| r.status.success())
}

/// 模拟器里真的 App（调试版，ios/test.sh 编、装的那一份）：扫 cca remote pair --web 给的链接配对（启动参数代替点界面，
/// 见 ios/App/TestDriver.swift）、列出 box 上的会话、接入、敲键，box 上的会话收到；最后截一张接着真会话的终端
fn in_the_simulator(t: &mut Ctx) {
    if std::env::var("CCA_IOS_SIM").ok().as_deref() != Some("1") {
        t.skip("模拟器里的端到端要先跑 bash ios/test.sh（开好模拟器、装上 App），再带 CCA_IOS_SIM=1");
    }
    let Some(udid) = simulator() else { t.skip("ios/test.sh 建的模拟器没开着") };
    // 服务器接着假的推送网关（kit::apnsfake）：App 登记推送、会话里的通知也推一份
    let gw = Gateway::start(&t.tmp("cca-push-"), Knobs::default());
    let push = vec!["--push".to_string(), gw.url(), "--push-pin".to_string(), gw.pin.clone()];
    let fx = Base::with(t, true, move |_| push);
    let a = fx.device("box");
    fx.machines(&[&a]);
    let token = Gateway::token("dd");
    let ios = harness::repo().join("ios").join("build");
    let app = ios.join("dd").join("Build").join("Products").join("Debug-iphonesimulator").join("cca.app");
    // 每次重新装：上一次的配对表、钥匙串都不留（App 第一次打开时清钥匙串）
    simctl(&["terminate", &udid, BUNDLE]);
    simctl(&["uninstall", &udid, BUNDLE]);
    assert!(simctl(&["install", &udid, &text(&app)]), "装不上 {}", app.display());
    let stop = udid.clone();
    t.defer(move || {
        simctl(&["terminate", &stop, BUNDLE]);
    });
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    let (mut p, link) = show(&a);
    let mut cmd = Cmd::new("xcrun");
    cmd.args(["simctl", "launch", "--console-pty", "--terminate-running-process", &udid, BUNDLE]);
    cmd.args([
        "-test-pair",
        &link,
        "-test-attach",
        "-test-type",
        "say from-the-iphone",
        "-test-note",
        "-test-wheel",
        "-test-sizes",
        "-test-push-only",
    ]);
    cmd.args(["-test-push-token", &token]);
    cmd.env_inherit().stdin(Stdio::Null).stdout(Stdio::Piped).stderr(Stdio::Null);
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{e}"));
    let lines = app_lines(child.stdout.take().expect("管道"));
    // 等 App 说某一句：先到的、还没问到的几句留着（登记推送的那一句和接入的几句谁先谁后不一定）
    let (mut seen, mut unread) = (Vec::new(), Vec::<String>::new());
    let mut said = |word: &str| -> String {
        if let Some(i) = unread.iter().position(|l| l.starts_with(word)) {
            return unread.remove(i);
        }
        let end = Instant::now() + secs(90);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match lines.recv_timeout(left) {
                Ok(line) => {
                    seen.push(line.clone());
                    if line.starts_with(word) {
                        return line;
                    }
                    if format!(" {line}").contains(" fail") || line.starts_with("state closed") {
                        break;
                    }
                    unread.push(line);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!("App 没说「{word}」：{seen:?}");
    };
    let pid = said("pair ok ").trim_start_matches("pair ok ").to_string();
    assert_eq!(p.wait(secs(60)), 0);
    let peers = relay::peers(&a);
    let rows: Vec<Value> = peers.obj().values().map(|r| relay::cols(r, &["name", "web"])).collect();
    assert_eq!(rows, [list!["我的 iPhone", true]]);
    // 通知扩展那样读（只在共用的钥匙串分组里找）：正好这个配对的一把通知密钥
    assert_eq!(said("keys "), format!("keys 1 {pid}"));
    // 在服务器上登记了推送：测试给的令牌，模拟器用的是开发环境的推送服务；「已关闭的会话也推送」关着（-test-push-only）：只推开着的
    let reg = said("push ");
    // 额度的通知默认开着：新的服务器认（usage）
    assert!(reg.ends_with(" ok only usage"), "{reg}");
    let guests = remote(&fx.d)["guests"].clone();
    let regs: Vec<Value> = guests.obj().values().map(|g| relay::cols(&g["push"], &["token", "env"])).collect();
    assert_eq!(regs, [list![&token, "sandbox"]], "{guests}");
    let sid = said("session ").trim_start_matches("session ").to_string();
    let mark = h.mark();
    said("state open");
    // 开了这个会话：登记跟着改成只推它（推送那一段要它）
    wait_for("登记里只推开着的会话", secs(30), || {
        remote(&fx.d)["guests"].obj().values().any(|g| g.get("push").and_then(|p| p.get("sids")) == Some(&list![&sid]))
    });
    // App 说自己能缩放：一接上会话就照手机排（列数是手机的；行数是两边都画得下的），App 的终端也照它排
    let re = regex::Regex::new(r"<size (\d+) (\d+)>").expect("正则");
    let end = h.expect_since(r"<size \d+ (\d|[0-7]\d)>", mark);
    let got = String::from_utf8_lossy(&h.out()[mark..end]).into_owned();
    let last = re.captures_iter(&got).last().expect("会话改了大小");
    said(&format!("shown {} {}", &last[2], &last[1]));
    said("typed");
    h.expect("<from-the-iphone>"); // box 上的会话收到了手机上敲的
    let mark = h.mark();

    // 会话里的钩子推一条通知：App 弹出来（调试版交给 TestDriver 说出来，不真的弹）；来自哪台机器是 App 自己写在副标题开头的
    h.send(format!("run {} remote-notify --title 做完了 --subtitle alice 第一行回复\n", relay::helper(&a).display()));
    let note = said("note ");
    assert!(note.starts_with("note 做完了|来自「") && note.ends_with("」 · alice|第一行回复"), "{note}");
    // 会话开了鼠标上报（像 Claude Code 的全屏界面）：手指往下拖三行是三格往上的滚轮（SGR 64），box 上的终端把它回显出来
    // 这边打了字（box 上的终端）：会话又照 box 上的终端排，App 缩小了画
    h.send(b"mouse\n");
    h.expect_since("<size 24 80>", mark);
    said("shown 80 24");
    said("wheel");
    let wheel_at = h.expect_since(r"(\^\[|\x1b)\[<64;\d+;\d+M(\^\[|\x1b)\[<64;\d+;\d+M(\^\[|\x1b)\[<64;\d+;\d+M", mark);
    std::thread::sleep(Duration::from_millis(1500));
    // 滚轮不算打字：会话的大小没跟着手机变
    assert!(!has(&h.out()[wheel_at..], "<size"), "{:?}", String::from_utf8_lossy(&h.out()[wheel_at..]));
    let shots = ios.join("shots");
    let _ = std::fs::create_dir_all(&shots);
    simctl(&["io", &udid, "screenshot", &text(&shots.join("e2e-terminal.png"))]);
    // 同一条通知也推了一份（宿主不知道手机在不在前台）：推送服务收到的那一份放进 App 的临时目录，App 像通知扩展那样解开
    let pushed = gw.sandbox.wait_seen(1, secs(30)).remove(0);
    assert_eq!(pushed.token(), token);
    let tmp = app_data(&udid).join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    fsx::write(tmp.join("pushed.part"), &pushed.body);
    std::fs::rename(tmp.join("pushed.part"), tmp.join("pushed.json")).unwrap_or_else(|e| panic!("{e}"));
    let card = said("pushed ");
    assert!(card.starts_with("pushed 做完了|来自「") && card.ends_with(&format!("」 · alice|第一行回复|{sid}")), "{card}");
    let _ = child.stop();
    drop(fx);
}

/// App 在模拟器里的数据目录（宿主机上的路径）
fn app_data(udid: &str) -> PathBuf {
    let mut cmd = Cmd::new("xcrun");
    cmd.args(["simctl", "get_app_container", udid, BUNDLE, "data"]).env_inherit().timeout(secs(60));
    let r = cmd.output().unwrap_or_else(|e| panic!("{e}"));
    assert!(r.status.success(), "{}", r.err());
    PathBuf::from(r.out().trim())
}

/// App 在控制台上说的、带 `cca-test: ` 的那几行（去掉前缀），一行一条送过来
fn app_lines(mut from: File) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut acc = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
            while let Some(nl) = acc.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&acc[..nl]).into_owned();
                acc.drain(..=nl);
                if let Some((_, said)) = line.split_once("cca-test: ") {
                    if tx.send(said.trim().to_string()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}
