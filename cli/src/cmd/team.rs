//! Team（docs/dev/team.md）：一组设备加一份配置（哪些账号、各走哪个节点、节点的定义），配置由管理员的设备维护、存在这个 Team 的
//! 同步服务器上（server/cca-presence.py --mode team）。这一份是设备这一头的子命令：拿邀请加入、取自己的那一份配置、退出、列出；
//! 管理员的（建 Team、接上服务器、邀请、授权、发布）在 team/admin.rs，线上格式在 team/doc.rs，本机的状态在 core/team.rs。
//!
//! 取回来的配置先验根密钥的签名（照加入时钉住的根公钥）、再认证解密，序号不比手里那一份大的不要，验过了才写进 doc.json。
//! 这一步还只是把配置取到本机：账号表照它对账是下一阶段的事（docs/dev/team.md 的阶段表）。

use super::cost::COST_DIR;
use super::hub::{hex32, host_name, host_of, keygen, name_ok, NAME_BAD};
use super::keyshare::is_num;
use super::presence::{flock, key_at, presence_key, presence_post, PRESENCE_DIR};
use super::router::device_id;
use super::{arity, Cmd};
use crate::core::base::Lock;
use crate::core::ctx::cfg_save;
use crate::core::model::cfg_load;
use crate::core::policy::policy_doc;
use crate::core::team::{
    forget_grants, grants, index_order, index_path, load_team, team_dir, team_doc, team_path, teams, Role, Team, TEAMS_DIR,
};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::{sha256, to_hex};
use crate::py::io::read_stdin_text;
use crate::py::obj::{str_list, Dict, V};
use crate::py::time::time;
use crate::py::{exit, json, path, text, Fail, R};
use crate::{dict, out};
use doc::{envelope_open, invite_decode, invite_parts, pub_proof, team_id, Invite};

mod admin;
pub mod doc;

/// 这个模块的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("team-join", cmd_team_join),
    Cmd::all("team-sync", cmd_team_sync),
    Cmd::all("team-leave", cmd_team_leave),
    Cmd::all("team-list", cmd_team_list),
    Cmd::all("team-apply", cmd_team_apply),
    Cmd::all("team-adopt", cmd_team_adopt),
    Cmd::all("team-new", admin::cmd_team_new),
    Cmd::all("team-attach", admin::cmd_team_attach),
    Cmd::all("team-deploy", admin::cmd_team_deploy),
    Cmd::all("team-hosted", admin::cmd_team_hosted),
    Cmd::all("team-invite", admin::cmd_team_invite),
    Cmd::all("team-handoff", admin::cmd_team_handoff),
    Cmd::all("team-devices", admin::cmd_team_devices),
    Cmd::all("team-revoke", admin::cmd_team_revoke),
    Cmd::all("team-grant", admin::cmd_team_grant),
    Cmd::all("team-rename", admin::cmd_team_rename),
    Cmd::all("team-import", admin::cmd_team_import),
    Cmd::all("team-map", admin::cmd_team_map),
    Cmd::all("team-alias", admin::cmd_team_alias),
    Cmd::all("team-name", admin::cmd_team_name),
    Cmd::all("team-publish", admin::cmd_team_publish),
];

/// 没成：stdout 是「原因码\t原因」，退出码 1（bash 里成了 FAIL_CODE=team_原因码）
fn fail<T>(code: &str, why: &str) -> R<T> {
    out!("{code}\t{why}");
    exit(1)
}

fn now() -> i64 {
    time().trunc() as i64
}

/// 公钥文件的前两段（ssh-ed25519 AAAA…）
fn pub_line(key: &str) -> R<String> {
    Ok(text::split_ws(&fs::read_text(&format!("{key}.pub"))?).into_iter().take(2).collect::<Vec<_>>().join(" "))
}

/// presence_post 用的配置：拿 key 这把私钥签名（设备的，或者管理员的根密钥）
fn cfg_of(url: &str, pins: &[String], key: String, fp: String) -> Dict {
    dict! { "url" => url, "pins" => str_list(pins), "key" => key, "fp" => fp }
}

/// 这台设备在这个 Team 里的配置（拿它自己的密钥签名）；没接上服务器、密钥不见了是给人看的原因
fn device_cfg(t: &Team) -> R<Result<Dict, String>> {
    if !t.attached() {
        return Ok(Err(format!("「{}」还没有接上同步服务器（cca team attach）", t.name)));
    }
    Ok(match key_at(&t.path("id_ed25519"))? {
        Some((key, fp)) => Ok(cfg_of(&t.url, &t.pins, key, fp)),
        None => Err(format!(
            "这台设备在「{}」里的密钥不见了（~/.cca/teams/{}/id_ed25519）：退出以后向管理员要一个新的邀请",
            t.name, t.id
        )),
    })
}

/// 请求没成的原因码和原因（url 是给人看的服务器地址）
fn fault(url: &str, code: i64, j: &Dict) -> (&'static str, String) {
    let err = j.str("error").map(|e| e.to_string()).unwrap_or_default();
    let host = host_of(url);
    match (code, err.as_str()) {
        (0, "pin") => {
            ("pin", "服务器的证书和钉住的指纹对不上（可能有人冒充服务器，也可能服务器换了证书：问一下 Team 的管理员）".into())
        }
        (0, "cert") => ("cert", format!("Team 的同步服务器（{host}）的证书验证不过：不是受信任的机构签的，或者和地址对不上")),
        (0, "sign") => ("sign", "没法用这台设备的密钥签名（ssh-keygen）".into()),
        (0, _) => ("unreachable", format!("连不上 Team 的同步服务器（{host}）")),
        (403, "unknown_device") => ("unknown", "Team 的同步服务器不认这台设备了".into()),
        (403, _) => ("denied", "Team 的同步服务器不认这个签名".into()),
        (404, _) => ("old", format!("{host} 不是 Team 的同步服务器（没有用 --mode team 运行），或者它的版本太旧")),
        (400, "clock") => {
            let gap = j.get("now").filter(is_num).and_then(|n| n.as_num()).map(|n| (time() - n).abs());
            let secs = gap.filter(|g| g.is_finite()).map_or(String::new(), |g| format!(" {:.0} 秒", g.trunc()));
            ("clock", format!("这台设备的时间和服务器差了{secs}：打开系统设置里日期与时间的自动设置"))
        }
        (429, _) => ("busy", "请求太频繁，稍后再试".into()),
        _ => ("server", format!("Team 的同步服务器出错了（HTTP {code}）")),
    }
}

/// 把 Team 记进 index.json（加入的先后；已经在的不动）。这台机器还没有 device_id 时顺便生成、写进 config.json：折合金额的密文里写着它
/// （dv），同一台机器在几台服务器上、退出又加入过都认得出是一台。有路由器的机器同步路由器时就有了（router.rs 的 device_id）；没有路由器的
/// 靠这里（3.5.2 是加入自己的同步服务器时生成的）
fn index_add(tid: &str) -> R<()> {
    let _lk = Lock::acquire()?;
    let c = cfg_load()?;
    if !c.truthy("device_id") {
        device_id(&c);
        cfg_save(&c)?;
    }
    let mut order = index_order();
    if !order.iter().any(|t| t == tid) {
        order.push(tid.to_string());
        save_json(&index_path(), &V::Dict(dict! { "v" => 1, "order" => str_list(&order) }), Some(0o600), true)?;
    }
    Ok(())
}

fn index_remove(tid: &str) -> R<()> {
    let _lk = Lock::acquire()?;
    let had = index_order();
    let order: Vec<String> = had.iter().filter(|t| *t != tid).cloned().collect();
    if order.len() != had.len() {
        save_json(&index_path(), &V::Dict(dict! { "v" => 1, "order" => str_list(&order) }), Some(0o600), true)?;
    }
    Ok(())
}

/// 这个 Team 在本机的文件一次只让一个进程改（取配置、管理员的命令）：拿着 teams/<编号>/lock 的锁。锁着的时候会联网（最多几十秒），
/// 所以不用全局的那一把（Lock）；同一个进程里别拿两次（flock 按打开的文件算：会等自己）
fn team_lock(tid: &str) -> R<std::fs::File> {
    let f = std::fs::OpenOptions::new().append(true).create(true).open(team_path(tid, "lock"))?;
    if !flock(&f, libc::LOCK_EX) {
        return Err(Fail::Die("锁不上这个 Team 的状态（teams/…/lock）".into()));
    }
    Ok(f)
}

fn save_team(tid: &str, j: &Dict) -> R<()> {
    save_json(&team_path(tid, "team.json"), &V::Dict(j.copy()), Some(0o600), true)
}

/// 本机记着的这个 Team 整个删掉（目录和 index.json 里的那一项）
fn forget(tid: &str) -> R<()> {
    let d = team_dir(tid);
    if path::isdir(&d) {
        fs::rmtree(&d)?;
    }
    drop_synced(tid);
    forget_grants();
    index_remove(tid)
}

/// 经这个 Team 的服务器同步时留在这台设备上的状态文件（presence/、cost/ 里 team.<编号>. 开头的）删掉：退出了、被管理员移除了以后
/// 不再读它们，而 cost 里那一份是解开了的别的设备的折合金额，不该留着
fn drop_synced(tid: &str) {
    let prefix = format!("team.{tid}.");
    for dir in [&*PRESENCE_DIR, &*COST_DIR] {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// 命令里写的是哪个 Team：编号、编号的开头（至少 4 个字符）或者名字；没写时只有一个就是它。找不到、不止一个是给人看的原因
fn find_team(sel: Option<&str>) -> Result<Team, String> {
    let all = teams();
    let Some(sel) = sel.filter(|s| !s.is_empty()) else {
        return match all.len() {
            0 => Err("这台设备还不在任何 Team 里（有邀请链接时运行 cca team join）".into()),
            1 => Ok(all.into_iter().next().expect("刚数过有一个")),
            _ => Err(format!("这台设备在 {} 个 Team 里，要写是哪一个（名字或者编号的开头）：{}", all.len(), names(&all))),
        };
    };
    let hits: Vec<Team> =
        all.iter().filter(|t| t.id == sel || t.name == sel || (sel.len() >= 4 && t.id.starts_with(sel))).cloned().collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().expect("刚数过有一个")),
        0 if all.is_empty() => Err("这台设备还不在任何 Team 里（有邀请链接时运行 cca team join）".into()),
        0 => Err(format!("没有叫「{sel}」的 Team（有：{}）", names(&all))),
        _ => Err(format!("「{sel}」不止一个 Team，写编号的开头：{}", names(&all))),
    }
}

fn names(all: &[Team]) -> String {
    all.iter().map(|t| format!("{}（{}）", t.name, &t.id[..8])).collect::<Vec<_>>().join("、")
}

// --- Team 的同步服务器（设备统计、记下的 API key、折合金额经它和这个 Team 里别的设备同步：hub.rs 的 hubs()）-------------------------------

/// 各个 Team 的同步服务器在这台设备上的配置：[(Team 的编号, presence_post 用的配置)]。配置比别的服务器的多 "hub": "team"、"team"（编号）、
/// "name"（Team 的名字）、"rev"（team.json 的修改时间：重新加入以后上报进程算新的一路）。接上了服务器、这台设备的密钥在、手里有验过的
/// 配置（盐在里面）、没被管理员移除的才有
pub fn hub_cfgs() -> Vec<(String, Dict)> {
    use std::os::unix::fs::MetadataExt;
    let grants = grants();
    let mut out = Vec::new();
    if grants.is_empty() {
        return out; // 没有 Team、都还没取到配置：不再读别的文件
    }
    for t in teams() {
        let Some(salt) = grants.iter().find(|g| g.id == t.id).and_then(|g| g.doc.str("salt")) else { continue };
        let cfg = match device_cfg(&t) {
            Ok(Ok(cfg)) => cfg,
            Ok(Err(_)) => continue,
            Err(e) => {
                e.swallowed(); // 这个 Team 的密钥文件读不了：只是它用不了，别的服务器照常
                continue;
            }
        };
        cfg.set("salt", &*salt);
        cfg.set("hub", "team");
        cfg.set("team", t.id.as_str());
        cfg.set("name", t.name.as_str());
        if let Ok(st) = std::fs::metadata(t.path("team.json")) {
            cfg.set("rev", st.mtime() as f64 + st.mtime_nsec() as f64 / 1e9);
        }
        out.push((t.id.clone(), cfg));
    }
    out
}

/// Team 的同步服务器的请求没成的原因（presence_why 遇到 "hub": "team" 的配置时用）
pub fn hub_why(cfg: &Dict, code: i64, j: &Dict) -> String {
    format!("「{}」的同步服务器：{}", cfg.at("name").py_str(), fault(&cfg.at("url").py_str(), code, j).1)
}

/// 这个 Team 给的账号现在不经它的服务器同步的原因（这台设备用不了它的服务器）
pub fn hub_missing_why(tid: &str) -> String {
    let Some(t) = load_team(tid) else { return "这台设备已经不在那个 Team 里了".into() };
    match sync_state(tid) {
        _ if !t.attached() => format!("「{}」还没有接上同步服务器", t.name),
        Some((s, why)) if s == "removed" => why,
        _ if team_doc(tid).is_none() => format!("「{}」的配置还没取到（cca team sync）", t.name),
        _ => format!("这台设备用不了「{}」的同步服务器（密钥不见了，或者关掉了设备统计）", t.name),
    }
}

// --- 取配置 ----------------------------------------------------------------------------------------------------------------

/// 这台设备那串随机数（邀请里的；管理员自己的那台是照主密钥算的）；没有、不合格是 None
fn secret_of(tid: &str) -> Option<String> {
    fs::read_text(&team_path(tid, "secret")).ok().map(|s| s.trim().to_string()).filter(|s| hex32(s))
}

/// sync.json 里的状态（还没取过是 None）
fn sync_state(tid: &str) -> Option<(String, String)> {
    let j = load_quiet(&team_path(tid, "sync.json"))?.as_dict()?;
    Some((j.str("state")?.to_string(), j.str("error").map(|e| e.to_string()).unwrap_or_default()))
}

/// 记下取配置的结果；只在变了时写（界面盯着这个文件）
fn sync_note(tid: &str, state: &str, error: &str) -> R<()> {
    if sync_state(tid).as_ref().map(|(s, e)| (s.as_str(), e.as_str())) != Some((state, error)) {
        let j = dict! { "v" => 1, "state" => state, "error" => error, "at" => now() };
        save_json(&team_path(tid, "sync.json"), &V::Dict(j), Some(0o600), true)?;
        forget_grants();
        if state == "removed" {
            drop_synced(tid);
        }
    }
    Ok(())
}

/// 取一次配置的结果：state 是 ok（手里的是最新的）| removed（被管理员移除了）| lost（服务器的记录没了，等管理员把这台设备登记回去）|
/// error（这次没取成，why 是原因）；seq 是手里那一份的序号（还没有是 0）
pub struct Synced {
    pub state: &'static str,
    pub seq: i64,
    pub why: String,
}

/// 向服务器取这台设备的配置，验过了换上（doc.json），结果记进 sync.json。拿着这个 Team 的锁、现读 team.json（别的进程可能刚改过）
pub fn sync_one(t: &Team) -> R<Synced> {
    let _lk = team_lock(&t.id)?;
    match load_team(&t.id) {
        Some(t) => sync_locked(&t),
        None => Ok(Synced { state: "error", seq: 0, why: "这个 Team 在本机的记录不见了".into() }),
    }
}

/// sync_one，调用方已经拿着这个 Team 的锁（管理员的命令、刚加入时）
fn sync_locked(t: &Team) -> R<Synced> {
    let have = team_doc(&t.id).map_or(0, |d| d.0);
    let r = sync_fetch(t, have)?;
    sync_note(&t.id, r.state, &r.why)?;
    Ok(r)
}

fn sync_fetch(t: &Team, have: i64) -> R<Synced> {
    let bad = |state: &'static str, why: String| Ok(Synced { state, seq: have, why });
    let cfg = match device_cfg(t)? {
        Ok(c) => c,
        Err(why) => return bad("error", why),
    };
    let Some(parts) = secret_of(&t.id).as_deref().and_then(invite_parts) else {
        return bad(
            "error",
            format!("这台设备在「{}」里的密钥不见了（~/.cca/teams/{}/secret）：退出以后向管理员要一个新的邀请", t.name, t.id),
        );
    };
    let payload = if have > 0 {
        dict! { "have" => have }
    } else {
        dict! {}
    };
    let (code, j) = presence_post(&cfg, "team_get", &payload, 15)?;
    if code == 403 && j.str("error").as_deref() == Some("unknown_device") {
        // 和加入时记下的编号一样是被移除了；不一样是服务器的记录没了（邀请只能用一次：等管理员照留着的公钥把这台设备登记回去）。
        // 这是服务器说的、没有签名：它能把两种说反，后果只是说法不对、被移除了的能拿新的邀请换上（见 cmd_team_join）
        let same = j.str("gen").is_some_and(|g| t.raw.str("gen").as_deref() == Some(&*g));
        return if same {
            bad("removed", format!("这台设备被「{}」的管理员移除了：要再加入，向管理员要一个新的邀请", t.name))
        } else {
            bad(
                "lost",
                format!("「{}」的同步服务器的记录没了：等管理员把这台设备登记回去（管理员运行 cca team devices 就会）", t.name),
            )
        };
    }
    if code != 200 {
        return bad("error", fault(&t.url, code, &j).1);
    }
    if j.str("dev").as_deref() != Some(t.dev.as_str()) {
        return bad("error", "服务器说的设备编号和这台设备的对不上：不用它给的配置".into());
    }
    let mut seq = have;
    let mut newer: Option<Dict> = None;
    if let Some(env) = j.dict("doc") {
        let (got, doc) = match envelope_open(&t.root, &parts.keys, &t.id, &t.dev, &env)? {
            Ok(x) => x,
            Err(why) => return bad("error", why),
        };
        if got < have {
            return bad("error", format!("服务器给的配置（第 {got} 版）比手里的（第 {have} 版）旧：不用它"));
        }
        // 一样的序号：手里的就是它，什么都不动（同一个序号的另一份内容——换服务器时原来那台上存的——也不照它改）
        if got > have {
            seq = got;
            newer = Some(doc);
        }
    }
    // 先照配置改 team.json（名字、服务器在哪），再写 doc.json：中间断了，下次手里还是旧的序号，服务器再给一次、接着做完
    follow(t, newer.as_ref(), &j)?;
    if let Some(doc) = newer {
        let saved = dict! { "v" => 1, "seq" => seq, "at" => now(), "doc" => doc };
        save_json(&t.path("doc.json"), &V::Dict(saved), Some(0o600), true)?;
        forget_grants();
    }
    Ok(Synced { state: "ok", seq, why: String::new() })
}

/// 新的一版配置里说的 Team 的名字、服务器在哪（管理员搬了服务器、换了证书），和服务器这一份记录的编号、这台设备在服务器上的名字
/// （管理员改了：cca team rename）：变了就跟着改 team.json。Team 的名字、服务器只照验过签名的新一版改；编号和设备的名字是服务器回的
/// （没有签名：编号只用来分清被移除了还是记录没了，设备的名字只是给人看的）
fn follow(t: &Team, doc: Option<&Dict>, reply: &Dict) -> R<()> {
    let raw = t.raw.copy();
    let mut changed = false;
    if let Some(doc) = doc {
        if let Some(n) = doc.str("name").filter(|n| **n != *t.name) {
            raw.set("name", &*n);
            changed = true;
        }
        if let Some(s) = doc.dict("server") {
            let (u, p) = (s.at("u").py_str(), s.list("p").unwrap_or_default());
            if u != t.url || !p.same(&str_list(&t.pins)) {
                raw.set("url", u);
                raw.set("pins", p);
                changed = true;
            }
        }
    }
    if let Some(g) = reply.str("gen").filter(|g| hex32(g) && raw.str("gen").as_deref() != Some(&**g)) {
        raw.set("gen", &*g);
        changed = true;
    }
    if let Some(n) = reply.str("me_name").filter(|n| name_ok(n) && **n != *t.device) {
        raw.set("device", &*n);
        changed = true;
    }
    if changed {
        save_team(&t.id, &raw)?;
    }
    Ok(())
}

/// 配置里给了这台设备几个账号
fn doc_accounts(doc: &Dict) -> usize {
    doc.dict("accounts").map_or(0, |a| a.values().iter().filter_map(|m| m.as_dict()).map(|m| m.len()).sum())
}

// --- 子命令 ----------------------------------------------------------------------------------------------------------------

/// 加入成了以后要说的：Team 的名字、服务器的地址:端口、这台设备在服务器上的名字、现在几台设备（服务器没说是空的）、Team 的编号、
/// 取配置的结果（ok | error…）、没取成时的原因（取成了是给了这台设备几个账号）
pub struct Joined {
    pub tid: String,
    pub name: String,
    pub host: String,
    pub device: String,
    pub members: String,
    pub state: &'static str,
    pub detail: String,
}

/// 拿邀请加入：成了是 Joined，没成是（原因码, 原因），没成时什么都不留。key_src：不新生成密钥，拷这把现成的（私钥的路径，旁边有
/// .pub）——照维护者的映射自己迁过来的机器拿拉映射用的 deploy key 当设备密钥，管理员据此认得是哪台；via：这个邀请是映射里来的
/// （它的摘要，记进 team.json 的 adopted：退出以后不再照同一个邀请自己加入）
fn join(inv: &Invite, name: &str, key_src: Option<&str>, via: Option<&str>) -> R<Result<Joined, (String, String)>> {
    let bad = |c: &str, why: &str| Ok(Err((c.to_string(), why.to_string())));
    let (Some(tid), Some(parts)) = (team_id(&inv.root), invite_parts(&inv.secret)) else {
        return bad("link", "邀请链接里根公钥不对：向 Team 的管理员要一个新的");
    };
    fs::makedirs_mode(&TEAMS_DIR, 0o700)?;
    match load_team(&tid) {
        Some(t) => {
            // 被移除了的再拿新的邀请加入：以前记着的不要了。别的情况不换（免得一条旧链接把正用着的换掉）
            let removed = sync_state(&tid).is_some_and(|(s, _)| s == "removed");
            if t.role == Role::Admin || !removed {
                return bad(
                    "joined",
                    &format!("这台设备已经在「{}」这个 Team 里了：要换成这个邀请，先运行 cca team leave", t.name),
                );
            }
        }
        // team.json 坏了、管理员的密钥还在：不能当成没有（删掉的话根密钥就没了）
        None if path::exists(&team_path(&tid, "root_ed25519")) || path::exists(&team_path(&tid, "admin.json")) => {
            return bad("joined", &format!("这台设备上有这个 Team 的管理员密钥，但 ~/.cca/teams/{tid}/team.json 读不了：先修好它"));
        }
        None => {}
    }
    forget(&tid)?;
    fs::makedirs_mode(&team_dir(&tid), 0o700)?;
    let _lk = team_lock(&tid)?;
    let kp = team_path(&tid, "id_ed25519");
    match key_src {
        Some(src) => {
            let pair = std::fs::read(src).and_then(|k| std::fs::read(format!("{src}.pub")).map(|p| (k, p)));
            if let Ok((k, p)) = pair {
                fs::put(&kp, &k, 0o600)?;
                fs::put(&format!("{kp}.pub"), &p, 0o644)?;
            }
        }
        None => keygen(&kp, "cca-team")?,
    }
    let Some((key, fp)) = key_at(&kp)? else {
        forget(&tid)?;
        return bad("sign", "生成不了这台设备的密钥（ssh-keygen）");
    };
    let cfg = cfg_of(&inv.url, &inv.pins, key, fp);
    let pubkey = pub_line(&kp)?;
    let payload = dict! { "pub" => pubkey.as_str(), "name" => name, "token" => parts.token.as_str() };
    if let Some(proof) = pub_proof(&inv.secret, &pubkey) {
        payload.set("proof", proof); // 管理员据此知道这把公钥真是拿着邀请的这台设备的
    }
    let (mut code, mut j) = presence_post(&cfg, "team_join", &payload, 15)?;
    // 说用过了、拿的又是现成的密钥（照映射迁过来的）：可能就是这台设备上一次用掉的——请求到了服务器、回话没收到。服务器认这把密钥是
    // 这个邀请的那台设备（取配置时回的编号对得上）就是已经加入了：把本机这一头补上
    if key_src.is_some() && code == 403 && j.str("error").as_deref() == Some("used") {
        let (c2, j2) = presence_post(&cfg, "team_get", &dict! {}, 15)?;
        if c2 == 200 && j2.str("dev").as_deref() == Some(parts.dev.as_str()) {
            code = 200;
            j = dict! { "dev" => parts.dev.as_str() };
            if let Some(g) = j2.get("gen") {
                j.set("gen", g);
            }
        }
    }
    let why: Option<(&str, String)> = match (code, j.str("error").as_deref()) {
        (200, _) if j.str("dev").as_deref() != Some(parts.dev.as_str()) => {
            Some(("server", "服务器回的设备编号和邀请里的对不上".into()))
        }
        (200, _) => None,
        (403, Some("expired")) => Some(("expired", "这个邀请过期了：向 Team 的管理员要一个新的".into())),
        (403, Some("used")) => Some((
            "used",
            "这个邀请已经用过了（一个邀请只能加入一台设备；不是你用的话告诉管理员）：向 Team 的管理员要一个新的".into(),
        )),
        (403, _) => Some(("denied", "服务器不认这个邀请（管理员撤回了，或者不是这台服务器的）：向 Team 的管理员要一个新的".into())),
        (409, _) => Some(("taken", "服务器上已经有这台设备了：让 Team 的管理员先移除它，再发一个新的邀请".into())),
        (507, _) => Some(("full", "这个 Team 的设备已经满了：让管理员先移除不用的".into())),
        (0, Some("unreachable")) => {
            // 请求可能已经到了服务器、只是回话没收到：那样的话邀请已经用掉了
            let (c, why) = fault(&inv.url, code, &j);
            Some((c, format!("{why}。再试一次；要是说邀请用过了，就是服务器其实收到了：让管理员移除这台设备、重新发邀请")))
        }
        _ => Some(fault(&inv.url, code, &j)),
    };
    if let Some((c, why)) = why {
        forget(&tid)?;
        return bad(c, &why);
    }
    let device = j.str("name").filter(|n| name_ok(n)).map_or(name.to_string(), |n| n.to_string());
    fs::put(&team_path(&tid, "secret"), format!("{}\n", inv.secret).as_bytes(), 0o600)?;
    let team = dict! {
        "v" => 1, "id" => tid.as_str(), "name" => inv.name.as_str(), "url" => inv.url.as_str(), "pins" => str_list(&inv.pins),
        "root" => inv.root.as_str(), "role" => Role::Member.name(), "dev" => parts.dev.as_str(), "device" => device.as_str(),
        "joined_at" => now(),
    };
    if let Some(g) = j.str("gen").filter(|g| hex32(g)) {
        team.set("gen", &*g);
    }
    if let Some(v) = via {
        team.set("adopted", v);
    }
    save_team(&tid, &team)?;
    index_add(&tid)?;
    let t = load_team(&tid).ok_or_else(|| Fail::Die("刚写好的 team.json 读不回来".into()))?;
    let s = sync_locked(&t)?;
    let t = load_team(&tid).unwrap_or(t);
    let members = j.get("members").and_then(|m| m.as_int()).filter(|m| *m > 0).map_or(String::new(), |m| m.to_string());
    let detail = if s.state == "ok" { team_doc(&tid).map_or(0, |d| doc_accounts(&d.1)).to_string() } else { s.why };
    Ok(Ok(Joined { tid, name: t.name.clone(), host: host_of(&t.url), device, members, state: s.state, detail }))
}

/// py team-join [--name 名字]：拿邀请加入一个 Team。邀请链接从 stdin 读（它是凭证：不进命令行）。成了：stdout 一行一项：Team 的名字、
/// 服务器的地址:端口、这台设备在服务器上的名字、现在几台设备（服务器没说是空行）、Team 的编号、取配置的结果（ok | error…）、
/// 没取成时的原因（取成了是给了这台设备几个账号），退出码 0。没成见 fail（原因码：link 链接不对、name 名字不对、joined 已经在这个
/// Team 里、sign、denied 服务器不认这个邀请、expired 过期了、used 用过了、taken 这把密钥已经是一台设备、full、pin、cert、
/// unreachable、old、clock、busy、server）。没成时什么都不留
pub fn cmd_team_join(args: &[String]) -> R<()> {
    let name = match args {
        [] => host_name(),
        [flag, n] if flag == "--name" => n.trim().to_string(),
        _ => return Err(Fail::Die("用法: py team-join [--name 名字]（邀请链接从 stdin 读）".into())),
    };
    if !name_ok(&name) {
        return fail("name", NAME_BAD);
    }
    let inv = match invite_decode(&read_stdin_text()) {
        Ok(i) => i,
        Err(why) => return fail("link", &why),
    };
    match join(&inv, &name, None, None)? {
        Ok(j) => {
            adopt_left(&j.tid, false)?;
            out!("{}\n{}\n{}\n{}\n{}\n{}\n{}", j.name, j.host, j.device, j.members, j.tid, j.state, j.detail);
            Ok(())
        }
        Err((c, why)) => fail(&c, &why),
    }
}

// --- 照维护者的映射自己迁过来（M4）-----------------------------------------------------------------------------------------------

/// 邀请链接的摘要（记「这个邀请试过了、退出过了」用：不存链接本身）
fn link_hash(link: &str) -> String {
    to_hex(&sha256(link.trim().as_bytes()))[..32].to_string()
}

fn adopt_path() -> String {
    path::join(&TEAMS_DIR, "adopt.json")
}

/// teams/adopt.json：{"v": 1, "links": {邀请的摘要: {"code": done（加入了）| 没成的原因码, "at": 什么时候}}（最多留 32 个）,
/// "left": {Team 的编号: 什么时候退出的}}
fn adopt_note(hash: &str, code: &str) -> R<()> {
    fs::makedirs_mode(&TEAMS_DIR, 0o700)?;
    let j = load_quiet(&adopt_path()).and_then(|j| j.as_dict()).unwrap_or_default();
    let links = j.dict("links").unwrap_or_default();
    links.set(hash, dict! { "code" => code, "at" => now() });
    let mut rows: Vec<(String, i64)> = links
        .items()
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_dict().and_then(|d| d.get("at")).and_then(|a| a.as_int()).unwrap_or(0)))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    for (k, _) in rows.iter().skip(32) {
        links.pop(k);
    }
    j.set("v", 1);
    j.set("links", links);
    save_json(&adopt_path(), &V::Dict(j), Some(0o600), true)
}

/// 记下（on）/ 去掉（off）「这台设备自己退出过这个 Team」：退出过的不再照映射里的邀请自己加入（管理员再发新的也不：要回来就自己
/// cca team join，那时去掉这个记号）。没有 adopt.json、没有这一项时去掉是什么都不做
fn adopt_left(tid: &str, on: bool) -> R<()> {
    let j = load_quiet(&adopt_path()).and_then(|j| j.as_dict());
    if !on && !j.as_ref().and_then(|j| j.dict("left")).is_some_and(|l| l.has(tid)) {
        return Ok(());
    }
    fs::makedirs_mode(&TEAMS_DIR, 0o700)?;
    let j = j.unwrap_or_default();
    let left = j.dict("left").unwrap_or_default();
    if on {
        left.set(tid, now());
    } else {
        left.pop(tid);
    }
    j.set("v", 1);
    j.set("left", left);
    save_json(&adopt_path(), &V::Dict(j), Some(0o600), true)
}

/// 维护者的映射里给这台设备的邀请：policy.json 的 "team": {"v": 1, "invites": {deploy key 的指纹: 邀请链接}}（维护者把各台机器迁到
/// Team 时 ./policy.sh team 写的）。映射验过发布密钥的签名才换上，所以这里的链接和映射一样可信。共享关着、没有这一节、没有这台设备
/// 的那一项是 None。返回（链接, 这台设备的 deploy key）
fn policy_invite() -> R<Option<(String, String)>> {
    let Some(j) = policy_doc() else { return Ok(None) };
    let Some(sec) = j.dict("team").filter(|s| s.get("v").is_some_and(|v| v.py_eq(&V::Int(1)))) else { return Ok(None) };
    let Some((key, fp)) = presence_key()? else { return Ok(None) };
    Ok(sec.dict("invites").and_then(|m| m.str(&fp)).map(|l| (l.to_string(), key)))
}

/// 照映射里的邀请加入（policy_invite）：这台设备还不在那个 Team 里（或者被移除了、映射里换了一个新的邀请），这个邀请没试过（上次是
/// 连不上、服务器出错、名额满了之类一时的原因、过了一个小时的再试）才加入，拿拉映射用的 deploy key 当设备密钥。自己退出过这个 Team 的
/// 不再自己加入。没成不出声，原因记在 teams/adopt.json（拉映射、取配置时在后台做的：下一次再试）。同时只做一个（后台拉映射、取配置
/// 可能撞上：另一个在做就算了）。加入了返回要说的那几项
pub fn adopt() -> R<Option<Joined>> {
    let Some((link, key)) = policy_invite()? else { return Ok(None) };
    let Ok(inv) = invite_decode(&link) else { return Ok(None) };
    let Some(tid) = team_id(&inv.root) else { return Ok(None) };
    let hash = link_hash(&link);
    fs::makedirs_mode(&TEAMS_DIR, 0o700)?;
    let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&TEAMS_DIR, "adopt.lock"))?;
    if !flock(&lock, libc::LOCK_EX | libc::LOCK_NB) {
        return Ok(None);
    }
    let state = load_quiet(&adopt_path()).and_then(|j| j.as_dict()).unwrap_or_default();
    if state.dict("left").is_some_and(|l| l.has(&tid)) {
        return Ok(None);
    }
    match load_team(&tid) {
        Some(t) => {
            let removed = sync_state(&tid).is_some_and(|(s, _)| s == "removed");
            if !(removed && t.role == Role::Member && t.raw.str("adopted").as_deref() != Some(&*hash)) {
                return Ok(None);
            }
        }
        None if path::exists(&team_dir(&tid)) => return Ok(None), // 建到一半、管理员的密钥还在：不碰
        None => {}
    }
    if let Some(e) = state.dict("links").and_then(|l| l.dict(&hash)) {
        let passing = matches!(
            e.str("code").as_deref(),
            Some("unreachable" | "pin" | "cert" | "busy" | "clock" | "old" | "sign" | "server" | "full")
        );
        let at = e.get("at").and_then(|a| a.as_int()).unwrap_or(0);
        if !passing || now() - at < 3600 {
            return Ok(None);
        }
    }
    let name = Some(host_name()).filter(|n| name_ok(n)).unwrap_or_else(|| "device".to_string());
    let r = join(&inv, &name, Some(&key), Some(&hash))?;
    adopt_note(&hash, r.as_ref().map_or_else(|(c, _)| c.as_str(), |_| "done"))?;
    Ok(r.ok())
}

/// py team-adopt：照维护者的映射里给这台设备的邀请加入（adopt）。加入了：stdout 一行「Team 的名字\t服务器的地址:端口\t这台设备在服务器
/// 上的名字\t给了它几个账号（没取到配置是 -）」；没有要加入的、没成：什么都不输出。退出码都是 0
pub fn cmd_team_adopt(args: &[String]) -> R<()> {
    arity("team-adopt", args, 0, 0)?;
    if let Some(j) = adopt()? {
        let n = if j.state == "ok" { j.detail.clone() } else { "-".to_string() };
        out!("{}\t{}\t{}\t{n}", j.name, j.host, j.device);
    }
    Ok(())
}

/// py team-sync [Team]：取一次配置（不写是哪个就每个都取）。stdout 一个 Team 一行：编号\t名字\t结果（ok | removed | lost | error）\t
/// 手里那一份的序号\t原因；要找的 Team 没有是「原因码\t原因」、退出码 1
pub fn cmd_team_sync(args: &[String]) -> R<()> {
    arity("team-sync", args, 0, 1)?;
    let list = match args.first() {
        Some(sel) => match find_team(Some(sel)) {
            Ok(t) => vec![t],
            Err(why) => return fail("no_team", &why),
        },
        None => {
            // 映射里有给这台设备的邀请就先加入（没成不出声；它出了错也不耽误取各个 Team 的配置）
            if let Err(e) = adopt() {
                e.swallowed();
            }
            teams()
        }
    };
    for t in list.iter().filter(|t| t.attached()) {
        let s = sync_one(t)?;
        out!("{}\t{}\t{}\t{}\t{}", t.id, load_team(&t.id).map_or(t.name.clone(), |n| n.name), s.state, s.seq, s.why);
    }
    Ok(())
}

/// py team-leave plan [Team] | apply 编号：退出一个 Team。plan（只读）：stdout 是「编号\t名字\t角色\t服务器的地址:端口」（bash 据此问同意：
/// 管理员的设备退出就是这台设备上不再有这个 Team 的根密钥）。apply：请服务器移除这台设备（连不上、服务器已经不认它了也接着退），
/// 本机记着的都删掉；stdout：Team 的名字、服务器的地址:端口（没接上是空行）、没告诉成服务器时的原因（告诉了是空行）。原因码：no_team
pub fn cmd_team_leave(args: &[String]) -> R<()> {
    let (apply, sel) = match args {
        [m] if m == "plan" => (false, None),
        [m, s] if m == "plan" => (false, Some(s.as_str())),
        [m, s] if m == "apply" => (true, Some(s.as_str())),
        _ => return Err(Fail::Die("用法: py team-leave plan [Team] | apply 编号".into())),
    };
    let t = match find_team(sel).and_then(|t| {
        if apply && Some(t.id.as_str()) != sel {
            Err("apply 要写完整的编号".into())
        } else {
            Ok(t)
        }
    }) {
        Ok(t) => t,
        Err(why) => return fail("no_team", &why),
    };
    let host = if t.attached() { host_of(&t.url) } else { String::new() };
    if !apply {
        out!("{}\t{}\t{}\t{host}", t.id, t.name, t.role.name());
        return Ok(());
    }
    let mut note = String::new();
    if t.attached() {
        match device_cfg(&t)? {
            Ok(cfg) => {
                let (code, j) = presence_post(&cfg, "team_leave", &dict! {}, 10)?;
                let gone = code == 403 && j.str("error").as_deref() == Some("unknown_device");
                if code != 200 && !gone {
                    note = format!("{}：服务器上还留着这台设备，让管理员移除它", fault(&t.url, code, &j).1);
                }
            }
            Err(why) => note = why,
        }
    }
    if t.role == Role::Member {
        adopt_left(&t.id, true)?; // 自己退出的：以后不照映射里的邀请自己加入这个 Team
    }
    forget(&t.id)?;
    out!("{}\n{host}\n{note}", t.name);
    Ok(())
}

/// py team-apply：照各个 Team 现在给的配置对一遍账号表（每条读账号表的命令本来都会做：这里只做这一件）。加了、固定了、收起了哪些
/// 账号写在 stderr（对账时的那几句），stdout 没有东西。bash 在加入、取到新配置、退出以后马上调它，再同步路由器、改启动器
pub fn cmd_team_apply(args: &[String]) -> R<()> {
    arity("team-apply", args, 0, 0)?;
    crate::core::ctx::ctx()?;
    Ok(())
}

/// 这台设备在的各个 Team（照加入的先后），每个是 team_row 的样子：cca team ls --json 的 teams、_gui state 的 config.teams
pub fn team_rows() -> Vec<V> {
    teams().iter().map(|t| V::Dict(team_row(t))).collect()
}

/// 一个 Team 在列表里的样子：state 是 local（管理员还没接上服务器）| pending（还没取过配置）| ok | removed | lost | error
fn team_row(t: &Team) -> Dict {
    let doc = team_doc(&t.id);
    let (state, error) = match sync_state(&t.id) {
        _ if !t.attached() => ("local".to_string(), String::new()),
        Some(s) => s,
        None => ("pending".to_string(), String::new()),
    };
    dict! {
        "id" => t.id.as_str(), "name" => t.name.as_str(), "role" => t.role.name(), "url" => t.url.as_str(),
        "host" => if t.attached() { V::from(host_of(&t.url)) } else { V::None }, "device" => t.device.as_str(),
        "state" => state, "error" => if error.is_empty() { V::None } else { V::from(error) },
        "seq" => doc.as_ref().map_or(0, |d| d.0), "accounts" => doc.as_ref().map_or(0, |d| doc_accounts(&d.1) as i64),
        "joined_epoch" => t.raw.get("joined_at").filter(is_num).unwrap_or(V::None),
    }
}

/// py team-list [--json]：这台设备在哪些 Team 里（只看本机记着的，不联网）。--json：{"version": 1, "kind": "team_list",
/// "teams": [team_row…]}；不带时一个 Team 一行：编号\t名字\t角色\t地址:端口\t状态\t序号\t几个账号\t原因
pub fn cmd_team_list(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return Err(Fail::Die("用法: py team-list [--json]".into())),
    };
    let rows: Vec<Dict> = teams().iter().map(team_row).collect();
    if as_json {
        let doc = dict! { "version" => 1, "kind" => "team_list", "teams" => rows.into_iter().map(V::Dict).collect::<Vec<V>>() };
        out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8().indent(2)));
        return Ok(());
    }
    for r in rows {
        // 没有的写 -：bash 按制表符读，空的栏会被并掉；还没有配置时序号、账号数是 0，也写 -
        let s = |k: &str| r.get(k).filter(|v| !v.is_none() && v.py_str() != "0").map_or("-".to_string(), |v| v.py_str());
        out!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            s("id"),
            s("name"),
            s("role"),
            s("host"),
            s("state"),
            s("seq"),
            s("accounts"),
            s("error")
        );
    }
    Ok(())
}
