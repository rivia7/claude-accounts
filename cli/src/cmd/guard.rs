//! Codex、Antigravity 的登录凭证防护（`py launch-args` 加在用户参数前面，accounts.rs 的 `cmd_launch_args`）：和 Claude 的
//! `cca_dir_args` 一样，挡住 ~/.cca 里的登录凭证和浏览器资料（`CCA_DIR_PRIVATE`）——
//! - Codex（`codex_guard_args`）：经 -c 定义一份权限配置（permissions）`cca`，扩展 Codex 这一次本来会用的沙箱（`codex_base`），
//!   文件系统里加上这几项 deny，再 -c default_permissions="cca" 选它。沙箱管的是 Codex 起的命令（cat、rg、sed 这些），Codex 自己
//!   读 auth.json 不受影响。同一个进程里经 Agent command center 开的别的会话也用这一份：它的沙箱照启动时的文件夹算
//! - agy（`agy_permissions`，agy.rs 的 `agy_launch_args` 用）：--settings 里的 permissions 照共享的 settings.json 那一份，deny 加上
//!   read_file(路径)
//!
//! 加不上的时候（用户自己指定了沙箱、不用沙箱、配置 cca 照搬不了……）不加，启动时说一句（bash 的 LAUNCH_NOTE）：不当作挡住了

use super::accounts::{toml_str, CCA_DIR_PRIVATE};
use super::codex::{codex_agent, codex_probe};
use super::settings::{read_toml_config, CODEX_VALUES};
use crate::core::base::CCA_HOME;
use crate::core::model::cdir;
use crate::py::fs;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::proc::env_or_empty;

// --- Codex -------------------------------------------------------------------------------------------------------------

/// cca 的权限配置的名字
const CODEX_PROFILE: &str = "cca";

/// Codex 0.172.0 直接用内置的 :workspace（信任过的文件夹、workspace-write）时工作目录里只读的子目录：扩展它的配置里没有这几项
/// （实测），cca 的要自己写上，不然比原来的松
const CODEX_WS_READ: [&str; 4] = [".git", ".agents", ".codex", ".aws"];

/// Codex 的路径里这些是通配符（只有 deny 认）：~/.cca 的真实路径、照搬的可写目录里有它们时写不准
const GLOB_CHARS: &[char] = &['*', '?', '[', ']', '{', '}'];

/// Codex 这一次本来会用的沙箱，cca 的权限配置扩展它
#[derive(Debug, PartialEq)]
enum Base {
    /// 扩展这一份：内置的 :read-only、:minimal，或者用户自己的权限配置（default_permissions 选的那一份）
    Extend(String),
    /// 内置的 :workspace（工作目录可写），再写上 CODEX_WS_READ。writable、network：[sandbox_workspace_write] 的 writable_roots、
    /// network_access（Codex 照老的规则选了 workspace-write 时照它们放开，cca 的配置照搬）
    Workspace { writable: Vec<String>, network: bool },
}

/// 用户参数里和沙箱有关的
#[derive(Debug, Default, PartialEq)]
struct Opts {
    /// 用户自己定了沙箱（-s / --sandbox、--dangerously-bypass-approvals-and-sandbox、-c 改沙箱的键）：不加，说一句
    own: Option<String>,
    /// --approve-for-me（--not-so-yolo）：Codex 换成 workspace-write
    approve: bool,
    /// exec --trust：在 git 仓库里时 Codex 先把这个仓库记成信任过，这一次就是 workspace-write（不在 git 仓库里时什么都不记）
    trust: bool,
    /// exec --ignore-user-config：Codex 不读 config.toml（-p 的那一份也不读；和 --trust 一起用时 Codex 报错退出）
    ignore_cfg: bool,
    /// -p / --profile：$CODEX_HOME/<名字>.config.toml 叠在 config.toml 上面
    profile: Option<String>,
    /// -C / --cd：会话的工作目录（相对的从当前目录算）
    cd: Option<String>,
}

/// 照 clap 认用户参数（0.172.0）：--x 值、--x=值、-x 值、-x值；带值的（settings.rs 的 CODEX_VALUES）跳过它的值，-- 后面的不看
fn codex_opts(args: &[String]) -> Opts {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            continue;
        }
        let (opt, mut val) = if arg.starts_with("--") {
            arg.split_once('=').map_or((arg, None), |(o, v)| (o, Some(v)))
        } else if arg.len() > 2 && arg.is_char_boundary(2) {
            let rest = &arg[2..]; // -cK=V、-pNAME、-CDIR、-sMODE：短选项后面直接跟着值
            (&arg[..2], Some(rest.strip_prefix('=').unwrap_or(rest)))
        } else {
            (arg, None)
        };
        if val.is_none() && CODEX_VALUES.contains(&opt) {
            val = args.get(i).map(String::as_str);
            i += 1;
        }
        match opt {
            "-s" | "--sandbox" => o.own = Some(format!("这次用 {opt} 指定了沙箱（Codex 照它选，不用 cca 的权限配置）")),
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => o.own = Some(format!("这次不用沙箱（{opt}）")),
            "--approve-for-me" | "--not-so-yolo" => o.approve = true,
            "--trust" => o.trust = true,
            "--ignore-user-config" => o.ignore_cfg = true,
            "-p" | "--profile" => o.profile = val.map(str::to_string),
            "-C" | "--cd" => o.cd = val.map(str::to_string),
            "-c" | "--config" => {
                let key = val.unwrap_or("").split('=').next().unwrap_or("").trim();
                let top = key.split('.').next().unwrap_or("").trim().trim_matches(['"', '\'']);
                if matches!(top, "sandbox_mode" | "sandbox_workspace_write" | "default_permissions" | "permissions") {
                    o.own = Some(format!("这次用 -c 改了 Codex 的沙箱（{key}）"));
                }
            }
            _ => {}
        }
    }
    o
}

/// 把 src 叠在 dst 上（表和表逐层合并，别的照 src 的），像 Codex 叠配置那样。src 和 dst 里原来的表都不改
fn merge(dst: &Dict, src: &Dict) {
    for (k, v) in src.items() {
        match (dst.get(&k), &v) {
            (Some(V::Dict(a)), V::Dict(b)) => {
                let c = a.copy();
                merge(&c, b);
                dst.set(k, c);
            }
            _ => dst.set(k, v),
        }
    }
}

/// 这一次 Codex 读的配置：账号的 config.toml（一般链到共享的那一份），叠上它的 profile 选的 [profiles.名字]（老的写法），再叠上
/// -p 的 <名字>.config.toml。读不了、写错了是 None（Codex 自己会报错）
fn codex_config(home: &str, profile: Option<&str>) -> Option<Dict> {
    let user = read_toml_config(&path::join(home, "config.toml")).ok()?;
    let cfg = Dict::new();
    merge(&cfg, &user);
    if let Some(p) = user.str("profile").and_then(|name| user.dict("profiles").and_then(|ps| ps.dict(&name))) {
        merge(&cfg, &p);
    }
    if let Some(name) = profile {
        merge(&cfg, &read_toml_config(&path::join(home, &format!("{name}.config.toml"))).ok()?);
    }
    Some(cfg)
}

/// 照 Codex 的规则（0.172.0 实测）算出这一次本来会用的沙箱。Err(Some(原因))：不加，说一句；Err(None)：不加也不说（配置写错了：
/// Codex 自己会报错）——
/// 1. --approve-for-me：workspace-write（盖过 sandbox_mode、default_permissions）
/// 2. default_permissions：选的那一份（`profile_base`）
/// 3. sandbox_mode：read-only、workspace-write；danger-full-access 不加
/// 4. 都没写：这个文件夹（-C 的或者当前目录；不然是它所在的 git 仓库的主目录）在 [projects] 里记过信任（trusted、untrusted 都算）
///    是 workspace-write，没记过是 read-only。在 git 仓库里时有两个例外：交互的会话 Codex 先问信不信任这个文件夹（不信任就回到
///    Agent command center，不开会话），信任了就是 workspace-write；exec --trust 先记下信任，也是 workspace-write
fn codex_base(cfg: &Dict, o: &Opts, interactive: bool, cwd: &str) -> Result<Base, Option<String>> {
    // 用户自己也有一份叫 cca 的（选没选它都一样）：-c 只换掉 extends、filesystem，它别的键（network 这些）会混进 cca 的这一份
    if cfg.dict("permissions").is_some_and(|p| p.has(CODEX_PROFILE)) {
        return Err(Some(format!("Codex 的配置里已经有一份叫 {CODEX_PROFILE} 的权限配置（[permissions.{CODEX_PROFILE}]）")));
    }
    if o.approve {
        return workspace(cfg);
    }
    match cfg.get("default_permissions") {
        Some(V::Str(name)) => return profile_base(&name, cfg),
        Some(_) => return Err(None),
        None => {}
    }
    match cfg.get("sandbox_mode") {
        Some(V::Str(m)) => {
            return match &*m {
                "read-only" => Ok(Base::Extend(":read-only".into())),
                "workspace-write" => workspace(cfg),
                "danger-full-access" => Err(Some("Codex 设成了不用沙箱（sandbox_mode = \"danger-full-access\"）".into())),
                _ => Err(None),
            }
        }
        Some(_) => return Err(None),
        None => {}
    }
    let repo = git_main_root(cwd);
    let projects = cfg.dict("projects");
    let marked = |p: &str| {
        let level = projects.as_ref().and_then(|ps| ps.dict(p)).and_then(|e| e.str("trust_level"));
        level.is_some_and(|l| matches!(&*l, "trusted" | "untrusted"))
    };
    if marked(cwd)
        || marked(&path::realpath(cwd))
        || repo.as_deref().is_some_and(marked)
        || ((interactive || o.trust) && repo.is_some())
    {
        workspace(cfg)
    } else {
        Ok(Base::Extend(":read-only".into()))
    }
}

/// default_permissions 选的那一份：顺着 extends 走到内置的（:read-only、:workspace、:minimal、:danger-full-access）为止。走到
/// :danger-full-access 的不加（不用沙箱，deny 也就不管用），绕回 cca 自己这一份的也不加（-c 会把它改成扩展它自己）。直接选内置的
/// :workspace 的照它（再写上 CODEX_WS_READ），别的扩展选的那一份
fn profile_base(name: &str, cfg: &Dict) -> Result<Base, Option<String>> {
    let perms = cfg.dict("permissions").unwrap_or_default();
    let mut cur = name.to_string();
    for _ in 0..32 {
        if cur == ":danger-full-access" {
            return Err(Some(format!("Codex 设成了不用沙箱（default_permissions = \"{name}\"）")));
        }
        if cur == CODEX_PROFILE {
            return Err(Some(format!("Codex 的配置里已经有一份叫 {CODEX_PROFILE} 的权限配置（default_permissions = \"{name}\"）")));
        }
        if cur.starts_with(':') {
            return Ok(match name {
                ":workspace" => Base::Workspace { writable: Vec::new(), network: false },
                _ => Base::Extend(name.to_string()),
            });
        }
        match perms.dict(&cur).map(|p| p.get("extends")) {
            Some(Some(V::Str(next))) => cur = next.to_string(),
            Some(None) => return Ok(Base::Extend(name.to_string())),
            _ => return Err(None), // 没定义这一份、extends 不是字符串：Codex 自己会报错
        }
    }
    Err(None) // extends 绕成了圈
}

/// workspace-write：[sandbox_workspace_write] 的 writable_roots、network_access 照搬；exclude_tmpdir_env_var、exclude_slash_tmp
/// 设成 true 的照搬不了，不加
fn workspace(cfg: &Dict) -> Result<Base, Option<String>> {
    let sww = match cfg.get("sandbox_workspace_write") {
        None => Dict::new(),
        Some(V::Dict(d)) => d,
        Some(_) => return Err(None),
    };
    for k in ["exclude_tmpdir_env_var", "exclude_slash_tmp"] {
        if sww.get(k).is_some_and(|v| v.as_bool() == Some(true)) {
            return Err(Some(format!("[sandbox_workspace_write] 的 {k} = true cca 照搬不了")));
        }
    }
    let mut writable = Vec::new();
    match sww.get("writable_roots") {
        None => {}
        Some(V::List(l)) => {
            for v in l.items() {
                let Some(p) = v.as_str().map(path::expanduser) else { return Err(None) };
                if !p.starts_with('/') || p.contains(GLOB_CHARS) {
                    return Err(Some(format!("[sandbox_workspace_write] 的 writable_roots 里有 cca 照搬不了的路径（{p}）")));
                }
                writable.push(p);
            }
        }
        Some(_) => return Err(None),
    }
    let network = sww.get("network_access").is_some_and(|v| v.as_bool() == Some(true));
    Ok(Base::Workspace { writable, network })
}

/// cwd 所在的 git 仓库的主目录（Codex 照 git rev-parse --git-common-dir 的上一层认信任；这里不起 git）：往上找 .git，是目录的就是
/// 这一层；是文件的（worktree）照里面的 gitdir 找到 commondir，主仓库的 .git 的上一层（子模块这些找不到 commondir 的就是这一层）
fn git_main_root(cwd: &str) -> Option<String> {
    let mut d = path::realpath(cwd);
    loop {
        let g = path::join(&d, ".git");
        if path::isdir(&g) {
            return Some(d);
        }
        if path::isfile(&g) {
            let text = fs::read_text(&g).ok()?;
            let gitdir = path::normpath(&path::join(&d, text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim()));
            let common = match fs::read_text(&path::join(&gitdir, "commondir")) {
                Ok(c) => path::normpath(&path::join(&gitdir, c.trim())),
                Err(_) => gitdir,
            };
            return Some(if path::basename(&common) == ".git" { path::dirname(&common) } else { d });
        }
        let up = path::dirname(&d);
        if up == d {
            return None;
        }
        d = up;
    }
}

/// CCA_DIR_PRIVATE 里的路径（相对 root），照写成 deny。collapse（Linux）：中间有一层现在还没有的目录时，整条换成那一层的 /**——
/// ~/.cca 在可写的目录里时（工作目录是 ~ 这些），同一个还没有的目录下有两条以上 deny，Codex 交给 bubblewrap 的挂载会冲突，整个
/// 沙箱起不来、什么命令都跑不了（0.172.0 实测，isolation.md）；换成的那一条挡住整个目录（之后建起来也挡）。有通配符的那一层往下
/// 不看（Codex 照现有的展开）
fn deny_paths(root: &str, collapse: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in CCA_DIR_PRIVATE {
        let mut entry = p.to_string();
        if collapse {
            let segs: Vec<&str> = p.split('/').collect();
            let mut cur = root.to_string();
            for (i, seg) in segs.iter().enumerate() {
                if seg.contains(GLOB_CHARS) {
                    break;
                }
                cur = path::join(&cur, seg);
                if !path::exists(&cur) {
                    if i + 1 < segs.len() {
                        entry = format!("{}/**", segs[..=i].join("/"));
                    }
                    break;
                }
            }
        }
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// cca 的权限配置：扩展本来的沙箱，CCA_DIR_PRIVATE 都是 deny（* 只管一层、结尾的 /** 管整个目录，和 Claude 的规则一样；collapse
/// 见 deny_paths），workspace-write 的再写上本来只读的子目录（CODEX_WS_READ）、照搬的可写目录和网络。-c 的值是 TOML：filesystem
/// 只能写成行内表（带引号的路径做不了点号分开的键）
fn codex_profile_args(root: &str, base: &Base, collapse: bool) -> Vec<String> {
    let mut fs: Vec<String> =
        deny_paths(root, collapse).iter().map(|p| format!("{} = \"deny\"", toml_str(&format!("{root}/{p}")))).collect();
    let (extends, network) = match base {
        Base::Extend(name) => (name.as_str(), false),
        Base::Workspace { writable, network } => {
            fs.extend(writable.iter().map(|w| format!("{} = \"write\"", toml_str(w))));
            let ro: Vec<String> = CODEX_WS_READ.iter().map(|d| format!("{} = \"read\"", toml_str(d))).collect();
            fs.push(format!("\":workspace_roots\" = {{{}}}", ro.join(", ")));
            (":workspace", *network)
        }
    };
    let p = CODEX_PROFILE;
    let mut out = vec![
        "-c".to_string(),
        format!("permissions.{p}.extends={}", toml_str(extends)),
        "-c".to_string(),
        format!("permissions.{p}.filesystem={{{}}}", fs.join(", ")),
    ];
    if network {
        out.extend(["-c".to_string(), format!("permissions.{p}.network={{enabled = true}}")]);
    }
    out.extend(["-c".to_string(), format!("default_permissions={}", toml_str(p))]);
    out
}

/// 探测用的（codex.rs 的 `codex_probe`，接着 features list）：defined 是 cca 那样定义、选中的一份（启动时的写法都带上：通配符的
/// deny、可写的目录、:workspace_roots、网络），读配置时要认；不然是选一份没定义的，要报错（不认 default_permissions 的旧版本
/// 照样能跑，就是没有这一项）
pub(crate) fn codex_probe_args(defined: bool) -> Vec<String> {
    if !defined {
        return vec!["-c".to_string(), format!("default_permissions={}", toml_str("cca-probe-undefined"))];
    }
    codex_profile_args(
        "/nonexistent-cca-probe",
        &Base::Workspace { writable: vec!["/nonexistent-cca-probe-w".into()], network: true },
        false,
    )
}

/// Codex 这一次加在用户参数前面的，和启动时说的一句（加不上的原因）。只加给干活的会话（交互的会话、exec、review：codex.rs 的
/// `codex_agent`）；这个版本的 codex 不认权限配置（`codex_probe` 的 perm）时不加也不说。Linux 上 Codex 的沙箱（bubblewrap）照样
/// 挡（0.172.0 在测试机上核对过：精确的路径、* 和 /**，已有的文件读、改、删都挡；往 deny 的目录里新建文件挡不住，parity.md），
/// 还没有的目录下的几条要合成一条（deny_paths 的 collapse）
pub fn codex_guard_args(key: &str, user: &[String]) -> (Vec<String>, Option<String>) {
    let Some(interactive) = codex_agent(user) else { return (Vec::new(), None) };
    if !codex_probe(&env_or_empty("CCA_BIN")).get("perm").is_some_and(|v| v.truthy()) {
        return (Vec::new(), None);
    }
    let o = codex_opts(user);
    let here = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let cwd = o.cd.as_deref().map_or_else(|| here.clone(), |d| path::abspath(&path::join(&here, &path::expanduser(d))));
    let root = path::realpath(&CCA_HOME);
    let base = match &o.own {
        Some(why) => Err(Some(why.clone())),
        None if root.contains(GLOB_CHARS) => Err(Some(format!("~/.cca 的路径里有通配符（{root}）"))),
        None => {
            let cfg = if o.ignore_cfg { Some(Dict::new()) } else { codex_config(&cdir(key), o.profile.as_deref()) };
            cfg.ok_or(None).and_then(|cfg| codex_base(&cfg, &o, interactive, &cwd))
        }
    };
    match base {
        Ok(base) => (codex_profile_args(&root, &base, cfg!(target_os = "linux")), None),
        Err(why) => (Vec::new(), why.map(|w| format!("~/.cca 里的登录凭证这次没给 Codex 挡住：{w}"))),
    }
}

// --- Antigravity -------------------------------------------------------------------------------------------------------

/// CCA_DIR_PRIVATE 照这台机器上现在有的展开成具体的路径（agy 的规则里文件、目录写全路径，目录连同里面的都算，通配符只有整条
/// 规则的 *）：结尾的 /** 去掉（就是那个目录），中间的 * 换成那一层现在有的目录（一个都没有就没有这一项），别的照写（还没有的
/// 文件、目录也挡）
fn private_paths(root: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in CCA_DIR_PRIVATE {
        let mut found = vec![root.to_string()];
        for seg in p.strip_suffix("/**").unwrap_or(p).split('/') {
            found = if seg == "*" {
                let kids = |d: &String| {
                    fs::listdir_sorted(d).unwrap_or_default().into_iter().map(|n| path::join(d, &n)).collect::<Vec<_>>()
                };
                found.iter().flat_map(kids).filter(|d| path::isdir(d)).collect()
            } else {
                found.iter().map(|d| path::join(d, seg)).collect()
            };
        }
        for f in found {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// agy 的 --settings 里的 permissions：共享的 settings.json 里用户自己的那一份（user；--settings 整个顶掉同名的顶层键），deny 加上
/// read_file(路径)（agy 1.5.18 的规则，Deny 在哪种模式下都算数；挡读也就挡了写）。Err 是不加的原因
pub fn agy_permissions(user: Option<V>) -> Result<Dict, String> {
    agy_permissions_in(&path::realpath(&CCA_HOME), user)
}

fn agy_permissions_in(root: &str, user: Option<V>) -> Result<Dict, String> {
    if root.contains(['(', ')']) {
        return Err(format!("~/.cca 的路径里有括号，规则写不准（{root}）"));
    }
    let perms = match user {
        None | Some(V::None) => Dict::new(),
        Some(V::Dict(d)) => d.copy(),
        Some(_) => return Err("共享的 settings.json 里的 permissions 不是对象".into()),
    };
    let deny = match perms.get("deny") {
        None | Some(V::None) => List::new(),
        Some(V::List(l)) => List::from_vec(l.items()),
        Some(_) => return Err("共享的 settings.json 里的 permissions.deny 不是数组".into()),
    };
    for p in private_paths(root) {
        let rule = V::str(format!("read_file({p})"));
        if !deny.contains(&rule) {
            deny.push(rule);
        }
    }
    perms.set("deny", deny);
    Ok(perms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::settings::toml_loads;
    use crate::py::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn toml(text: &str) -> Dict {
        toml_loads(text).unwrap()
    }

    /// 测试用的空目录（真实路径），用完删掉
    struct Tmp(String);
    impl Tmp {
        fn new(name: &str) -> Tmp {
            let d = format!("{}/cca-guard-{}-{name}", std::env::temp_dir().display(), std::process::id());
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Tmp(path::realpath(&d))
        }
        fn mk(&self, rel: &str) -> String {
            let d = path::join(&self.0, rel);
            std::fs::create_dir_all(&d).unwrap();
            d
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opts_follow_clap() {
        // -m 的值是 -s（不算沙箱）
        let o = codex_opts(&s(&["-m", "-s", "exec", "--cd=/w", "-pfast"]));
        assert_eq!(o, Opts { profile: Some("fast".into()), cd: Some("/w".into()), ..Opts::default() });
        for own in [
            &["exec", "-s", "read-only", "hi"][..],
            &["--sandbox=workspace-write"],
            &["-sread-only"],
            &["--yolo"],
            &["--dangerously-bypass-approvals-and-sandbox"],
            &["-c", "sandbox_workspace_write.network_access=true"],
            &["--config=permissions.x.extends=\":workspace\""],
            &["-c'default_permissions'=\":read-only\""],
        ] {
            assert!(codex_opts(&s(own)).own.is_some(), "{own:?}");
        }
        assert!(codex_opts(&s(&["-c", "model=\"o3\"", "--", "-s"])).own.is_none());
        assert!(codex_opts(&s(&["--not-so-yolo"])).approve);
        let o = codex_opts(&s(&["exec", "--trust", "--ignore-user-config", "hi"]));
        assert!(o.trust && o.ignore_cfg && o.own.is_none());
        assert_eq!(codex_opts(&s(&["-C", "sub", "exec", "-C/x"])).cd.as_deref(), Some("/x"));
    }

    #[test]
    fn base_follows_codex() {
        let t = Tmp::new("base");
        let repo = t.mk("repo");
        t.mk("repo/.git");
        let sub = t.mk("repo/sub");
        let plain = t.mk("plain");
        let ro = Ok(Base::Extend(":read-only".into()));
        let ws = Ok(Base::Workspace { writable: Vec::new(), network: false });
        let o = Opts::default();
        let empty = Dict::new();
        // 没记过信任：read-only；交互的会话在 git 仓库里（Codex 先问信不信任，信任了才开会话）是 workspace-write
        assert_eq!(codex_base(&empty, &o, false, &sub), ro);
        assert_eq!(codex_base(&empty, &o, true, &plain), ro);
        assert_eq!(codex_base(&empty, &o, true, &sub), ws);
        // exec --trust：在 git 仓库里时 Codex 先记下信任（workspace-write），不在时什么都不记
        let trust = Opts { trust: true, ..Opts::default() };
        assert_eq!(codex_base(&empty, &trust, false, &sub), ws);
        assert_eq!(codex_base(&empty, &trust, false, &plain), ro);
        // 用户自己也有一份叫 cca 的权限配置（没选它也不行：-c 只换掉 extends、filesystem）
        let clash = toml("[permissions.cca]\nextends = \":read-only\"\n[permissions.cca.network]\nenabled = true\n");
        assert!(matches!(codex_base(&clash, &o, false, &plain), Err(Some(_))));
        // 记过信任（trusted、untrusted 都算）：文件夹本身，或者它所在的 git 仓库的主目录；不在 git 仓库里时上层记过的不算
        let marked = toml(&format!("[projects.{}]\ntrust_level = \"untrusted\"\n", toml_str(&repo)));
        assert_eq!(codex_base(&marked, &o, false, &sub), ws);
        let parent = toml(&format!("[projects.{}]\ntrust_level = \"trusted\"\n", toml_str(&t.0)));
        assert_eq!(codex_base(&parent, &o, false, &plain), ro);
        let own = toml(&format!("[projects.{}]\ntrust_level = \"trusted\"\n", toml_str(&plain)));
        assert_eq!(codex_base(&own, &o, false, &plain), ws);
        // sandbox_mode（[sandbox_workspace_write] 照搬）、--approve-for-me（盖过 sandbox_mode）
        let cfg = toml(
            "sandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nnetwork_access = true\nwritable_roots = [\"/opt/x\"]\n",
        );
        assert_eq!(codex_base(&cfg, &o, false, &plain), Ok(Base::Workspace { writable: s(&["/opt/x"]), network: true }));
        assert!(matches!(codex_base(&toml("sandbox_mode = \"danger-full-access\"\n"), &o, false, &plain), Err(Some(_))));
        let afm = Opts { approve: true, ..Opts::default() };
        assert_eq!(codex_base(&toml("sandbox_mode = \"read-only\"\n"), &afm, false, &plain), ws);
        let tmp = toml("sandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nexclude_slash_tmp = true\n");
        assert!(matches!(codex_base(&tmp, &o, false, &plain), Err(Some(_))));
        let rel = toml("sandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nwritable_roots = [\"rel\"]\n");
        assert!(matches!(codex_base(&rel, &o, false, &plain), Err(Some(_))));
        // default_permissions：扩展选的那一份；顺着 extends 到 :danger-full-access 的、绕回 cca 的不加；没定义的不说（Codex 自己报错）
        let named = toml("default_permissions = \"mine\"\n[permissions.mine]\nextends = \":workspace\"\n");
        assert_eq!(codex_base(&named, &o, true, &sub), Ok(Base::Extend("mine".into())));
        assert_eq!(codex_base(&toml("default_permissions = \":workspace\"\n"), &o, false, &plain), ws);
        assert_eq!(
            codex_base(&toml("default_permissions = \":minimal\"\n"), &o, false, &plain),
            Ok(Base::Extend(":minimal".into()))
        );
        let wild = toml(
            "default_permissions = \"a\"\n[permissions.a]\nextends = \"b\"\n[permissions.b]\nextends = \":danger-full-access\"\n",
        );
        assert!(matches!(codex_base(&wild, &o, false, &plain), Err(Some(_))));
        let same =
            toml("default_permissions = \"a\"\n[permissions.a]\nextends = \"cca\"\n[permissions.cca]\nextends = \":read-only\"\n");
        assert!(matches!(codex_base(&same, &o, false, &plain), Err(Some(_))));
        assert_eq!(codex_base(&toml("default_permissions = \"nope\"\n"), &o, false, &plain), Err(None));
        let cycle = toml("default_permissions = \"a\"\n[permissions.a]\nextends = \"b\"\n[permissions.b]\nextends = \"a\"\n");
        assert_eq!(codex_base(&cycle, &o, false, &plain), Err(None));
    }

    #[test]
    fn config_layers() {
        let t = Tmp::new("layers");
        std::fs::write(
            path::join(&t.0, "config.toml"),
            "profile = \"old\"\nsandbox_mode = \"read-only\"\n[profiles.old]\nsandbox_mode = \"workspace-write\"\n[sandbox_workspace_write]\nnetwork_access = true\n",
        )
        .unwrap();
        std::fs::write(path::join(&t.0, "fast.config.toml"), "sandbox_mode = \"danger-full-access\"\n").unwrap();
        let cfg = codex_config(&t.0, None).unwrap();
        assert_eq!(cfg.str("sandbox_mode").as_deref(), Some("workspace-write"));
        assert_eq!(cfg.dict("sandbox_workspace_write").and_then(|d| d.get("network_access")).and_then(|v| v.as_bool()), Some(true));
        let cfg = codex_config(&t.0, Some("fast")).unwrap();
        assert_eq!(cfg.str("sandbox_mode").as_deref(), Some("danger-full-access"));
        std::fs::write(path::join(&t.0, "config.toml"), "sandbox_mode = \n").unwrap();
        assert!(codex_config(&t.0, None).is_none());
    }

    #[test]
    fn worktree_trust_follows_main_repo() {
        let t = Tmp::new("wt");
        let main = t.mk("main");
        t.mk("main/.git/worktrees/wt");
        let wt = t.mk("wt");
        let inner = t.mk("wt/src");
        std::fs::write(path::join(&wt, ".git"), format!("gitdir: {main}/.git/worktrees/wt\n")).unwrap();
        std::fs::write(path::join(&main, ".git/worktrees/wt/commondir"), "../..\n").unwrap();
        assert_eq!(git_main_root(&inner), Some(main.clone()));
        assert_eq!(git_main_root(&main), Some(main.clone()));
    }

    #[test]
    fn profile_args_deny_private_paths() {
        let a = codex_profile_args("/h/.cca", &Base::Extend(":read-only".into()), false);
        assert_eq!(a[..2], s(&["-c", "permissions.cca.extends=\":read-only\""])[..]);
        assert!(a[3].starts_with("permissions.cca.filesystem={\"/h/.cca/policy/id_ed25519\" = \"deny\", "), "{}", a[3]);
        assert!(a[3].contains(", \"/h/.cca/accounts/*/chrome/**\" = \"deny\", "));
        assert!(a[3].ends_with("\"/h/.cca/agy/accounts/*/chrome/**\" = \"deny\"}"), "{}", a[3]);
        assert_eq!(a[4..], s(&["-c", "default_permissions=\"cca\""])[..]);
        let w = codex_profile_args("/h/.cca", &Base::Workspace { writable: s(&["/opt/x"]), network: true }, false);
        assert_eq!(w[1], "permissions.cca.extends=\":workspace\"");
        let ro =
            "\":workspace_roots\" = {\".git\" = \"read\", \".agents\" = \"read\", \".codex\" = \"read\", \".aws\" = \"read\"}}";
        assert!(w[3].ends_with(&format!(", \"/opt/x\" = \"write\", {ro}")), "{}", w[3]);
        assert_eq!(w[4..], s(&["-c", "permissions.cca.network={enabled = true}", "-c", "default_permissions=\"cca\""])[..]);
        // 探测用的两份：定义了、选中的一份（启动时的写法都在里面），选一份没定义的
        assert!(codex_probe_args(true).iter().any(|x| x.contains("\"/nonexistent-cca-probe-w\" = \"write\"")));
        assert_eq!(codex_probe_args(false), s(&["-c", "default_permissions=\"cca-probe-undefined\""]));
    }

    #[test]
    fn linux_collapses_missing_dirs() {
        // 不合的时候（macOS、探测）照 CCA_DIR_PRIVATE 原样写
        assert_eq!(deny_paths("/nonexistent-cca", false), s(CCA_DIR_PRIVATE));
        // 合的时候（Linux）：中间一层还没有的整条换成那一层的 /**、去重；最后一层没有的（文件）、有通配符的照写
        let t = Tmp::new("collapse");
        for d in ["hub", "core", "codex/accounts/a1/codex"] {
            t.mk(d);
        }
        let got = deny_paths(&t.0, true);
        let want = s(&[
            "policy/**",
            "hub/hub.json",
            "hub/id_ed25519",
            "hub-hosted.json",
            "teams/**",
            "codex/accounts/*/codex/auth.json",
            "accounts/**",
            "codex/accounts/*/chrome/**",
            "codex/accounts/*/desktop/**",
            "core/nodes.json",
            "core/custom-nodes.json",
            "core/config.yaml",
            "secrets/**",
            "agy/**",
        ]);
        assert_eq!(got, want);
        // 都在的时候和不合一样
        for d in ["policy", "teams", "accounts", "secrets", "agy/accounts"] {
            t.mk(d);
        }
        assert_eq!(deny_paths(&t.0, true), s(CCA_DIR_PRIVATE));
        let a = codex_profile_args(&t.0, &Base::Extend(":read-only".into()), true);
        assert!(a[3].contains(&format!("\"{}/codex/accounts/*/codex/auth.json\" = \"deny\"", t.0)), "{}", a[3]);
    }

    #[test]
    fn private_patterns_are_whole_segments() {
        // private_paths 只认整段的 *（和结尾的 /**）
        for p in CCA_DIR_PRIVATE {
            for seg in p.strip_suffix("/**").unwrap_or(p).split('/') {
                assert!(seg == "*" || !seg.contains('*'), "{p}");
            }
        }
    }

    #[test]
    fn agy_rules_expand_private_paths() {
        let t = Tmp::new("agy");
        let r = t.0.clone();
        for d in ["accounts/a1/chrome", "accounts/a2", "agy/accounts/g1/gemini/antigravity-cli"] {
            t.mk(d);
        }
        std::fs::write(path::join(&r, "accounts/not-a-dir"), "").unwrap();
        let user =
            json::loads(&format!(r#"{{"allow": ["read_file(*)"], "deny": ["command(rm)", "read_file({r}/teams)"], "ask": []}}"#))
                .unwrap();
        let p = agy_permissions_in(&r, Some(user)).unwrap();
        assert_eq!(p.keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), s(&["allow", "deny", "ask"]));
        assert_eq!(p.list("allow").unwrap().strs().len(), 1);
        let deny: Vec<String> = p.list("deny").unwrap().strs().iter().map(|x| x.to_string()).collect();
        let rule = |x: &str| format!("read_file({r}/{x})");
        assert_eq!(deny[..3], [s(&["command(rm)"]), vec![rule("teams")], vec![rule("policy/id_ed25519")]].concat()[..]);
        for x in ["accounts/a1/chrome", "accounts/a2/chrome", "accounts/a2/desktop", "secrets", "core/config.yaml"] {
            assert!(deny.contains(&rule(x)), "{x}");
        }
        assert!(deny.contains(&rule("agy/accounts/g1/gemini/antigravity-cli/antigravity-oauth-token")));
        assert!(!deny.iter().any(|x| x.contains("not-a-dir") || x.contains("codex/accounts") || x.contains('*')), "{deny:?}");
        assert_eq!(deny.iter().filter(|x| **x == rule("teams")).count(), 1);
        // 没有 permissions 时只有 deny；permissions、deny 不是对象、数组时不加；~/.cca 的路径里有括号时不加
        let p = agy_permissions_in(&r, None).unwrap();
        assert_eq!(p.keys().len(), 1);
        assert!(agy_permissions_in(&r, Some(V::str("x"))).is_err());
        assert!(agy_permissions_in(&r, Some(json::loads(r#"{"deny": "x"}"#).unwrap())).is_err());
        assert!(agy_permissions_in("/h/a(b)", None).is_err());
    }
}
